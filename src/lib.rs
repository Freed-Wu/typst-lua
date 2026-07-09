use mlua::prelude::*;
use typst::foundations::{Array, Dict, Str, Value as TypstValue};
use typst_as_library::{self, OutputFormat};

// -------------------------------------
// TRAIT: FromLuaTypst
// -------------------------------------

trait FromLuaTypst {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue>;
}

// Implement for LuaValue
impl FromLuaTypst for LuaValue {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue> {
        match self {
            LuaValue::Nil => Ok(TypstValue::None),

            LuaValue::Boolean(b) => Ok(TypstValue::Bool(b)),
            LuaValue::Number(n) => Ok(TypstValue::Float(n)),
            LuaValue::Integer(n) => Ok(TypstValue::Int(n)),

            LuaValue::String(s) => match s.to_str() {
                Ok(text) => Ok(TypstValue::Str(Str::from(text.to_string()))),
                Err(_) => {
                    let bytes_vec: Vec<u8> = s.as_bytes().to_vec();
                    Ok(TypstValue::Bytes(typst::foundations::Bytes::new(bytes_vec)))
                }
            },

            LuaValue::Table(t) => t.to_typst(lua),

            LuaValue::UserData(ud) => {
                return Err(LuaError::RuntimeError(
                    "Lua userdata cannot be converted to Typst value".into(),
                ));
            }

            other => Err(LuaError::RuntimeError(format!(
                "Unsupported Lua value: {other:?}"
            ))),
        }
    }
}

// -------------------------------------
// Implement for Lua Table (no lifetime)
// -------------------------------------

impl FromLuaTypst for LuaTable {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue> {
        // First pass: check if this is an array
        let mut is_array = true;
        let mut expected = 1;
        let mut count = 0;

        for pair in self.pairs::<LuaValue, LuaValue>() {
            let (key, _) = pair?;
            count += 1;

            match key {
                LuaValue::Integer(idx) => {
                    if idx != expected {
                        is_array = false;
                        break;
                    }
                    expected += 1;
                }
                LuaValue::Number(n) if n.fract() == 0.0 => {
                    let idx = n as i64;
                    if idx != expected {
                        is_array = false;
                        break;
                    }
                    expected += 1;
                }
                _ => {
                    is_array = false;
                    break;
                }
            }
        }

        // Second pass: populate the appropriate data structure
        if is_array {
            let mut arr = Array::new();
            for pair in self.pairs::<LuaValue, LuaValue>() {
                let (_, value) = pair?;
                arr.push(value.to_typst(lua)?);
            }
            Ok(TypstValue::Array(arr))
        } else {
            let mut map = Dict::new();
            for pair in self.pairs::<LuaValue, LuaValue>() {
                let (key, value) = pair?;
                let v = value.to_typst(lua)?;

                let key_str = match key {
                    LuaValue::Integer(idx) => Str::from(idx.to_string()),
                    LuaValue::Number(n) if n.fract() == 0.0 => Str::from((n as i64).to_string()),
                    LuaValue::String(s) => Str::from(s.to_str()?.to_owned()),
                    LuaValue::Boolean(b) => Str::from(b.to_string()),
                    other => {
                        return Err(LuaError::RuntimeError(format!(
                            "Unsupported Lua table key: {other:?}"
                        )))
                    }
                };

                map.insert(key_str, v);
            }
            Ok(TypstValue::Dict(map))
        }
    }
}

// -------------------------------------
// Compile function exposed to Lua
// -------------------------------------

/// Dispatch helper: accepts either:
/// - Old style: `typst.compile("file.typ", data_table)`
/// - New style: `typst.compile{ file="file.typ", input=..., format="html", ppi=144 }`
fn compile(
    lua: &Lua,
    args: LuaMultiValue,
) -> LuaResult<(Option<LuaString>, Option<LuaString>)> {
    let mut iter = args.into_iter();
    match iter.next() {
        // New table-based API: typst.compile{ file=..., format=..., ... }
        Some(LuaValue::Table(t)) => compile_from_table(lua, t),
        // Legacy API: typst.compile("file.typ", data)
        Some(LuaValue::String(s)) => {
            let data = iter.next().unwrap_or(LuaValue::Nil);
            compile_legacy(lua, s, data)
        }
        other => {
            let err = lua.create_string(
                "typst-lua: compile() expects a table or a file-path string as first argument"
            )?;
            Ok((None, Some(err)))
        }
    }
}

fn compile_legacy(
    lua: &Lua,
    input: LuaString,
    data: LuaValue,
) -> LuaResult<(Option<LuaString>, Option<LuaString>)> {
    let input_text = input.to_str()?.to_string();

    let typst_value_opt = match data {
        LuaValue::Table(_) => {
            match data.to_typst(lua) {
                Ok(val) => Some(val),
                Err(e) => {
                    let err_msg = lua.create_string(&format!(
                        "typst-lua: error converting lua table to typst value: {e}"
                    ))?;
                    return Ok((None, Some(err_msg)));
                }
            }
        }
        _ => None,
    };

    let pdf_bytes = match typst_as_library::compile(&input_text, &typst_value_opt) {
        Ok(bytes) => bytes,
        Err(e) => {
            let err_msg = lua.create_string(&format!("typst: {e}"))?;
            return Ok((None, Some(err_msg)));
        }
    };

    let pdf = lua.create_string(&pdf_bytes)?;
    Ok((Some(pdf), None))
}

/// Table-based compile API:
/// ```lua
/// typst.compile{
///   file   = "file.typ",
///   input  = { key = "value" },  -- optional sys.inputs dict
///   format = "pdf",              -- "pdf" | "html" | "svg" | "png"
///   ppi    = 144.0,              -- optional, only for "png" (default 144.0)
/// }
/// ```
/// Returns `(bytes, err)` where `bytes` is a Lua string and `err` is nil on
/// success, or `(nil, err_string)` on failure.
fn compile_from_table(
    lua: &Lua,
    args: LuaTable,
) -> LuaResult<(Option<LuaString>, Option<LuaString>)> {
    // Required: file path
    let file: String = match args.get::<LuaValue>("file")? {
        LuaValue::String(s) => s.to_str()?.to_string(),
        other => {
            let err = lua.create_string(&format!(
                "typst-lua: 'file' must be a string, got {other:?}"
            ))?;
            return Ok((None, Some(err)));
        }
    };

    // Optional: input dict
    let typst_value_opt = match args.get::<LuaValue>("input")? {
        LuaValue::Nil => None,
        LuaValue::Table(_) => {
            let v: LuaValue = args.get("input")?;
            match v.to_typst(lua) {
                Ok(val) => Some(val),
                Err(e) => {
                    let err_msg = lua.create_string(&format!(
                        "typst-lua: error converting 'input' to typst value: {e}"
                    ))?;
                    return Ok((None, Some(err_msg)));
                }
            }
        }
        other => {
            let err = lua.create_string(&format!(
                "typst-lua: 'input' must be a table or nil, got {other:?}"
            ))?;
            return Ok((None, Some(err)));
        }
    };

    // Optional: format (default "pdf")
    let format_str: String = match args.get::<LuaValue>("format")? {
        LuaValue::Nil => "pdf".to_string(),
        LuaValue::String(s) => s.to_str()?.to_lowercase(),
        other => {
            let err = lua.create_string(&format!(
                "typst-lua: 'format' must be a string or nil, got {other:?}"
            ))?;
            return Ok((None, Some(err)));
        }
    };

    // Optional: ppi for PNG (default 144.0)
    let ppi: f32 = match args.get::<LuaValue>("ppi")? {
        LuaValue::Nil => 144.0,
        LuaValue::Number(n) => n as f32,
        LuaValue::Integer(n) => n as f32,
        other => {
            let err = lua.create_string(&format!(
                "typst-lua: 'ppi' must be a number or nil, got {other:?}"
            ))?;
            return Ok((None, Some(err)));
        }
    };

    let output_format = match format_str.as_str() {
        "pdf" => OutputFormat::Pdf,
        "html" => OutputFormat::Html,
        "svg" => OutputFormat::Svg,
        "png" => OutputFormat::Png { ppi },
        other => {
            let err = lua.create_string(&format!(
                "typst-lua: unsupported format '{other}'; expected 'pdf', 'html', 'svg', or 'png'"
            ))?;
            return Ok((None, Some(err)));
        }
    };

    let bytes = match typst_as_library::compile_with_format(&file, &typst_value_opt, output_format) {
        Ok(b) => b,
        Err(e) => {
            let err_msg = lua.create_string(&format!("typst: {e}"))?;
            return Ok((None, Some(err_msg)));
        }
    };

    let result = lua.create_string(&bytes)?;
    Ok((Some(result), None))
}

// -------------------------------------
// Module Export
// -------------------------------------

#[mlua::lua_module]
fn typst(lua: &Lua) -> LuaResult<LuaTable> {
    let exports = lua.create_table()?;
    exports.set("compile", lua.create_function(compile)?)?;
    Ok(exports)
}
