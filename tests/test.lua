local socket = require "socket"
package.cpath = "./?.so;" .. package.cpath
local typst = require "typst"

local output_dir = "output"
local data_dir   = "data"

local function join(...)
    local t = {...}
    return table.concat(t, "/")
end

local function write_output(bytes, outpath)
    local fh = assert(io.open(outpath, "wb"))
    fh:write(bytes)
    fh:close()
end

local function load_data(data_file)
    if not data_file then return nil end
    local path = join(data_dir, data_file)
    return assert(loadfile(path, "t", _ENV)())
end

local function test_compile(opts, should_error)
    local name = opts.file .. " [" .. (opts.format or "pdf") .. "]"
    local t0 = socket.gettime()
    local bytes, err = typst.compile(opts)
    local ms = (socket.gettime() - t0) * 1000

    if should_error then
        assert(err, "Expected compilation error but got none for " .. name)
        assert(not bytes, "Expected no output for " .. name)
        print(string.format("OK: %s errored as expected (%.2f ms)", name, ms))
        print("Error: " .. err)
    else
        assert(not err, "Compilation error for " .. name .. ": " .. tostring(err))
        assert(bytes and #bytes > 0, "Empty output for " .. name)
        local ext = opts.format or "pdf"
        write_output(bytes, join(output_dir, opts.file:match("[^/]+$") .. "." .. ext))
        print(string.format("OK: %s (%.2f ms)", name, ms))
    end
end

-- Tests
test_compile({ file = join("templates", "test_error.typ"), input = load_data("test_typ_extended.lua") }, true)
test_compile{ file = join("templates", "test_blank.typ") }
test_compile{ file = join("templates", "test.typ"), input = load_data("test_typ_extended.lua") }
test_compile{ file = join("templates", "test_decode.typ"), input = load_data("decoded_image.lua") }
test_compile{ file = join("templates", "test_extended.typ"), input = load_data("test_typ_extended.lua") }
test_compile{ file = join("templates", "test_download.typ") }
test_compile{ file = join("templates", "test_pdfinclusion.typ"), input = load_data("test_pdf_inclusion.lua") }

-- Multi-format tests
test_compile{ file = join("templates", "test_blank.typ"), format = "pdf" }
test_compile{ file = join("templates", "test_blank.typ"), format = "html" }
test_compile{ file = join("templates", "test_blank.typ"), format = "svg" }
test_compile{ file = join("templates", "test_blank.typ"), format = "png", ppi = 144 }
test_compile{
    file   = join("templates", "test.typ"),
    input  = load_data("test_typ_extended.lua"),
    format = "html",
}
