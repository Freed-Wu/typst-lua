use mlua::prelude::*;
use typst::foundations::{Array, Dict, Str, Value as TypstValue};
use typst_as_library::OutputFormat;

// -------------------------------------
// TRAIT: FromLuaTypst
// -------------------------------------

trait FromLuaTypst {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue>;
}

impl FromLuaTypst for LuaValue {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue> {
        match self {
            LuaValue::Nil => Ok(TypstValue::None),
            LuaValue::Boolean(b) => Ok(TypstValue::Bool(b)),
            LuaValue::Number(n) => Ok(TypstValue::Float(n)),
            LuaValue::Integer(n) => Ok(TypstValue::Int(n)),
            LuaValue::String(s) => match s.to_str() {
                Ok(text) => Ok(TypstValue::Str(Str::from(text.to_string()))),
                Err(_) => Ok(TypstValue::Bytes(typst::foundations::Bytes::new(
                    s.as_bytes().to_vec(),
                ))),
            },
            LuaValue::Table(t) => t.to_typst(lua),
            LuaValue::UserData(_) => Err(LuaError::RuntimeError(
                "Lua userdata cannot be converted to Typst value".into(),
            )),
            other => Err(LuaError::RuntimeError(format!(
                "Unsupported Lua value: {other:?}"
            ))),
        }
    }
}

impl FromLuaTypst for LuaTable {
    fn to_typst(self, lua: &Lua) -> LuaResult<TypstValue> {
        // First pass: check if this is a sequential array
        let mut is_array = true;
        let mut expected = 1;

        for pair in self.pairs::<LuaValue, LuaValue>() {
            let (key, _) = pair?;
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

/// ```lua
/// typst.compile{
///   file   = "file.typ",
///   input  = { key = "value" },  -- optional sys.inputs dict
///   format = "pdf",              -- "pdf" | "html" | "svg" | "png"
///   ppi    = 144.0,              -- optional, only for "png" (default 144.0)
/// }
/// ```
/// Returns `(bytes, err)`: `bytes` is a Lua string on success, `err` is a
/// string on failure.
fn compile(
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

    Ok((Some(lua.create_string(&bytes)?), None))
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
