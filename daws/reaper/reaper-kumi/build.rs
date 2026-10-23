// Generates the REAPER API table from the vendored SDK headers (sdk/SOURCE.txt) and builds the control surface shim.
//
// reaper_plugin_functions.h declares each function on one line, `ret (*REAPERAPI_FUNCNAME(Name))(args);`, and
// that line is all we read: the function's name, its C signature (kept as text for the generic call) and a typed
// Rust function pointer, resolved at load time through GetFunc. Nothing is listed by hand, so a newer header
// brings its new functions with it. The header's own loader table names every function a second time: a
// prototype this parser can't read fails the build there, by name, instead of going missing.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

const FUNCTIONS_HEADER: &str = "sdk/reaper_plugin_functions.h";
const PLUGIN_HEADER: &str = "sdk/reaper_plugin.h";
const SHIM: &str = "shim/csurf.cpp";
/// The preview listening plays out loud (M5): REAPER's preview_register_t, made where the OS's lock type is known.
const PREVIEW_SHIM: &str = "shim/preview.cpp";

fn main() {
    println!("cargo:rerun-if-changed={FUNCTIONS_HEADER}");
    println!("cargo:rerun-if-changed={PLUGIN_HEADER}");
    println!("cargo:rerun-if-changed={SHIM}");
    println!("cargo:rerun-if-changed={PREVIEW_SHIM}");
    println!("cargo:rerun-if-changed=build.rs");

    let functions_header = fs::read_to_string(FUNCTIONS_HEADER).expect("read reaper_plugin_functions.h");
    let plugin_header = fs::read_to_string(PLUGIN_HEADER).expect("read reaper_plugin.h");
    let shim = fs::read_to_string(SHIM).expect("read the shim");
    let preview_shim = fs::read_to_string(PREVIEW_SHIM).expect("read the preview shim");

    let functions = parse_functions(&functions_header);
    check_loader_table(&functions_header, &functions);
    let sdk_version = generated_by(&functions_header);
    let mut out = String::new();
    emit(&mut out, &sdk_version, &functions, &plugin_header);
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("reaper_api.rs"), out).expect("write reaper_api.rs");
    fs::write(out_dir.join("reaper_generic.rs"), emit_generic(&functions)).expect("write reaper_generic.rs");

    check_shim(&plugin_header, &shim);
    check_preview(&plugin_header, &preview_shim);
    build_shim(&[Path::new(SHIM), Path::new(PREVIEW_SHIM)]);
    // The name the library gives itself on macOS: the linker's default is the absolute path it was built at. REAPER
    // loads it by path, so this only keeps the build folder out of the file Kumi ships.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/reaper_kumi.dylib");
    }
}

struct Function {
    name: String,
    ret: String,
    params: Vec<Param>,
}

struct Param {
    ty: String,
    name: String,
    // A function pointer: passed as an untyped pointer.
    callback: bool,
}

fn parse_functions(header: &str) -> Vec<Function> {
    const MARK: &str = "(*REAPERAPI_FUNCNAME(";
    let mut functions = Vec::new();
    for line in header.lines() {
        let Some(at) = line.find(MARK) else { continue };
        // The loader table at the end mentions every name too, as `{(void**)&REAPERAPI_FUNCNAME(x),"x"}`.
        if !line.starts_with("  ") || line.trim_start().starts_with('{') {
            continue;
        }
        let ret = line[..at].trim().to_string();
        let rest = &line[at + MARK.len()..];
        let close = rest.find("))(").unwrap_or_else(|| panic!("unexpected prototype: {line}"));
        let name = rest[..close].to_string();
        let args = rest[close + 3..].trim_end().strip_suffix(");").unwrap_or_else(|| panic!("unexpected prototype: {line}"));
        let params = split_top_level(args).into_iter().filter(|p| !p.is_empty() && p != "void").map(|p| parse_param(&p)).collect();
        functions.push(Function { name, ret, params });
    }
    assert!(functions.len() > 800, "only {} prototypes found in {FUNCTIONS_HEADER}", functions.len());
    let names: BTreeSet<_> = functions.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names.len(), functions.len(), "a function is declared twice");
    functions
}

// REAPERAPI_LoadAPI's table, `{(void**)&REAPERAPI_FUNCNAME(Name),"Name"},`: every function the header declares,
// listed apart from the prototypes. Each one must have been read from its prototype, and nothing else.
fn check_loader_table(header: &str, functions: &[Function]) {
    const ENTRY: &str = "{(void**)&REAPERAPI_FUNCNAME(";
    let mut listed = BTreeSet::new();
    for line in header.lines() {
        let Some(rest) = line.trim_start().strip_prefix(ENTRY) else { continue };
        let (name, quoted) = rest.split_once("),\"").unwrap_or_else(|| panic!("unexpected loader entry: {line}"));
        assert_eq!(quoted.strip_suffix("\"},"), Some(name), "a loader entry names two functions: {line}");
        assert!(listed.insert(name.to_string()), "{name} is in the loader table twice");
    }
    let parsed: BTreeSet<String> = functions.iter().map(|f| f.name.clone()).collect();
    let unread: Vec<_> = listed.difference(&parsed).collect();
    let unlisted: Vec<_> = parsed.difference(&listed).collect();
    assert!(unread.is_empty(), "{FUNCTIONS_HEADER} declares functions build.rs couldn't read from their prototypes: {unread:?}");
    assert!(unlisted.is_empty(), "build.rs read prototypes the loader table doesn't list: {unlisted:?}");
}

fn split_top_level(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in args.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    parts.push(current.trim().to_string());
    parts
}

fn parse_param(param: &str) -> Param {
    if let Some(open) = param.find("(*") {
        let name = param[open + 2..].split(')').next().unwrap().trim().to_string();
        return Param { ty: param.to_string(), name, callback: true };
    }
    let split = param.rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or_else(|| panic!("unnamed parameter: {param}"));
    let ty = param[..=split].trim().to_string();
    let name = param[split + 1..].to_string();
    assert!(!name.is_empty() && !ty.is_empty(), "unexpected parameter: {param}");
    Param { ty, name, callback: false }
}

fn generated_by(header: &str) -> String {
    let line = header.lines().find(|l| l.contains("Generated by REAPER v")).expect("the header's REAPER version");
    line.split("Generated by REAPER v").nth(1).unwrap().trim().to_string()
}

// C types that are numbers or handles; any other name is an opaque REAPER type.
fn scalar(base: &str) -> Option<&'static str> {
    Some(match base {
        "void" => "c_void",
        "bool" => "bool",
        "char" => "c_char",
        "int" => "c_int",
        "unsigned int" | "UINT" => "c_uint",
        "float" => "f32",
        "double" => "f64",
        "INT64" => "i64",
        "INT_PTR" => "isize",
        "size_t" => "usize",
        "LICE_pixel" => "u32",
        "LICE_pixel_chan" => "u8",
        "HWND" | "HMENU" | "HDC" | "HFONT" | "HINSTANCE" | "screensetNewCallbackFunc" => "*mut c_void",
        _ => return None,
    })
}

fn rust_type(c: &str, opaque: &mut BTreeSet<String>) -> String {
    let c = c.trim();
    let stars = c.matches('*').count();
    let base = c.trim_end_matches(['*', ' ']);
    let (constant, base) = match base.strip_prefix("const ") {
        Some(rest) => (true, rest.trim()),
        None => (false, base),
    };
    assert!(!base.contains('*') && !base.contains("const"), "unsupported C type: {c}");
    let mut ty = match scalar(base) {
        Some(t) => t.to_string(),
        None => {
            assert!(base.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'), "unsupported C type: {c}");
            assert!(stars > 0, "a REAPER type by value: {c}");
            opaque.insert(base.to_string());
            base.to_string()
        }
    };
    for level in 0..stars {
        ty = if level == 0 && constant { format!("*const {ty}") } else { format!("*mut {ty}") };
    }
    if stars == 0 && base == "void" {
        ty = "()".to_string();
    }
    ty
}

fn emit(out: &mut String, sdk_version: &str, functions: &[Function], plugin_header: &str) {
    let mut opaque = BTreeSet::new();
    let mut accessors = String::new();
    let mut table = String::new();
    for (index, f) in functions.iter().enumerate() {
        let params: Vec<String> =
            f.params.iter().map(|p| if p.callback { "*mut c_void".to_string() } else { rust_type(&p.ty, &mut opaque) }).collect();
        let ret = rust_type(&f.ret, &mut opaque);
        let ret = if ret == "()" { String::new() } else { format!(" -> {ret}") };
        let signature = format!("unsafe extern \"C\" fn({}){ret}", params.join(", "));
        writeln!(accessors, "    /// `{} {}({})`", f.ret, f.name, f.params.iter().map(c_param).collect::<Vec<_>>().join(", ")).unwrap();
        writeln!(accessors, "    pub fn {}(&self) -> Option<{signature}> {{", f.name).unwrap();
        writeln!(accessors, "        let p = self.ptr({index});").unwrap();
        writeln!(
            accessors,
            "        if p.is_null() {{ None }} else {{ Some(unsafe {{ std::mem::transmute::<*mut c_void, {signature}>(p) }}) }}"
        )
        .unwrap();
        writeln!(accessors, "    }}").unwrap();
        let params = f.params.iter().map(|p| format!("Param {{ ty: {:?}, name: {:?} }}", p.ty, p.name)).collect::<Vec<_>>().join(", ");
        writeln!(table, "    Function {{ name: c{:?}, ret: {:?}, params: &[{params}] }},", f.name, f.ret).unwrap();
    }

    writeln!(out, "// Generated by build.rs from sdk/reaper_plugin_functions.h and sdk/reaper_plugin.h. Don't edit.").unwrap();
    writeln!(out, "/// The REAPER version that generated the vendored header.").unwrap();
    writeln!(out, "pub const SDK_VERSION: &str = {sdk_version:?};").unwrap();
    writeln!(out, "pub const FUNCTION_COUNT: usize = {};", functions.len()).unwrap();
    for (name, value) in defines(plugin_header) {
        writeln!(out, "pub const {name}: i32 = {value};").unwrap();
    }
    for name in &opaque {
        writeln!(out, "/// REAPER's `{name}`, only ever behind a pointer.").unwrap();
        writeln!(out, "#[repr(C)]\npub struct {name} {{ _opaque: [u8; 0] }}").unwrap();
    }
    writeln!(out, "/// Every API function, in the header's order: the index of each accessor below.").unwrap();
    writeln!(out, "pub static FUNCTIONS: [Function; {}] = [\n{table}];", functions.len()).unwrap();
    writeln!(out, "impl Api {{\n{accessors}}}").unwrap();
}

fn c_param(p: &Param) -> String {
    if p.callback {
        p.ty.clone()
    } else {
        format!("{} {}", p.ty, p.name)
    }
}

// REAPER_PLUGIN_VERSION and the CSURF_EXT_* codes.
fn defines(header: &str) -> Vec<(String, String)> {
    let mut defines = Vec::new();
    for line in header.lines() {
        let Some(rest) = line.strip_prefix("#define ") else { continue };
        let mut words = rest.split_whitespace();
        let (Some(name), Some(value)) = (words.next(), words.next()) else { continue };
        if name == "REAPER_PLUGIN_VERSION" || name.starts_with("CSURF_EXT_") {
            let digits = value.strip_prefix("0x").expect("a hex define");
            let value = i64::from_str_radix(digits, 16).expect("a hex define");
            defines.push((name.to_string(), value.to_string()));
        }
    }
    assert!(defines.len() > 20, "the CSURF_EXT_* codes are missing from {PLUGIN_HEADER}");
    defines
}

// The shim declares IReaperControlSurface itself (the SDK header needs SWELL on macOS and Linux), so its virtual
// functions must be the header's, in the header's order: that order is the vtable REAPER calls through.
fn check_shim(plugin_header: &str, shim: &str) {
    let header = virtuals(plugin_header, "class IReaperControlSurface");
    let ours = virtuals(shim, "class IReaperControlSurface");
    assert!(header.len() > 20, "IReaperControlSurface not found in {PLUGIN_HEADER}");
    assert_eq!(ours, header, "{SHIM}'s IReaperControlSurface differs from {PLUGIN_HEADER}'s");
}

fn virtuals(source: &str, class: &str) -> Vec<String> {
    let start = source.find(class).unwrap_or_else(|| panic!("{class} not found"));
    let body = &source[start..];
    let end = body.find("};").expect("the class's end");
    body[..end]
        .lines()
        .map(|l| l.split("//").next().unwrap().trim())
        .filter(|l| l.starts_with("virtual"))
        .map(|l| {
            let decl = l.split('{').next().unwrap();
            let decl = decl.split("=0").next().unwrap();
            decl.split_whitespace().collect::<Vec<_>>().join(" ").replace(" *", "* ").replace("* ", "*").trim_end_matches(';').to_string()
        })
        .collect()
}

// The preview shim declares preview_register_t itself (the header needs SWELL outside Windows): its fields must be the
// header's, in the header's order, since REAPER reads the struct Kumi hands it.
fn check_preview(plugin_header: &str, shim: &str) {
    let header = fields(plugin_header, "typedef struct _REAPER_preview_register_t");
    let ours = fields(shim, "typedef struct _REAPER_preview_register_t");
    assert!(header.len() >= 8, "preview_register_t not found in {PLUGIN_HEADER}");
    assert_eq!(ours, header, "{PREVIEW_SHIM}'s preview_register_t differs from {PLUGIN_HEADER}'s");
}

fn fields(source: &str, start: &str) -> Vec<String> {
    let at = source.find(start).unwrap_or_else(|| panic!("{start} not found"));
    let body = &source[at..];
    let open = body.find('{').expect("the struct's body");
    let end = body.find("} preview_register_t;").expect("the struct's end");
    let mut out = Vec::new();
    let mut in_comment = false;
    for line in body[open + 1..end].lines() {
        let mut line = line.trim();
        if in_comment {
            match line.find("*/") {
                Some(close) => {
                    in_comment = false;
                    line = line[close + 2..].trim();
                }
                None => continue,
            }
        }
        if line.starts_with("/*") {
            in_comment = !line.contains("*/");
            continue;
        }
        let line = line.split("//").next().unwrap().trim();
        if !line.is_empty() {
            out.push(line.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    out
}

fn build_shim(shims: &[&Path]) {
    let mut build = cc::Build::new();
    build.cpp(true).warnings(true);
    for shim in shims {
        build.file(shim);
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // REAPER's C++ interfaces need the MSVC ABI; a static CRT means no VC++ runtime to install. cc follows the
        // target feature (/MT), so the release build passes -C target-feature=+crt-static.
        let features = env::var("CARGO_CFG_TARGET_FEATURE").unwrap_or_default();
        if !features.split(',').any(|f| f == "crt-static") {
            println!("cargo:warning=reaper_kumi.dll links the VC++ runtime dynamically; release builds use -C target-feature=+crt-static");
        }
    } else {
        build.flag("-std=c++11").flag("-fno-exceptions").flag("-fno-rtti").flag("-fvisibility=hidden").flag("-Wno-unused-parameter");
    }
    build.compile("reaper_kumi_shim");
}

// ----- the generic call: every function the API table has, called by name with JSON arguments -----
//
// Each parameter is read from the call's arguments by what its C type and name say it is: a number or a flag, text, a
// buffer REAPER writes into (with its size, which the caller never passes), a buffer whose size REAPER reads and writes
// back (`int* name_sz`: made as big as that size says), a value REAPER writes back (`…Out`), a handle given as a ref (a
// track, item, take, envelope or project). A function with any other parameter or return (callbacks, windows, bitmaps,
// sources, arrays REAPER fills, binary data passed with its size) can't be called this way: run-lua can.

const HANDLES: [&str; 5] = ["MediaTrack", "MediaItem", "MediaItem_Take", "TrackEnvelope", "ReaProject"];

/// The flags that make a GetSet… function set (generic.rs's `reads_only` reads the same names).
const SET_FLAGS: [&str; 7] = ["setNewValue", "setnewvalue", "set", "is_set", "isSet", "bSet", "setNewValueOptional"];

/// A value REAPER only writes back (`…Out`, `…OutOptional`), never reads.
fn out_only(name: &str) -> bool {
    !name.contains("InOut") && (name.ends_with("Out") || name.contains("OutOptional"))
}

fn scalar_kind(base: &str) -> Option<&'static str> {
    Some(match base {
        "int" | "char" => "int",
        "unsigned int" | "UINT" => "uint",
        "INT64" => "int64",
        "INT_PTR" => "isize",
        "size_t" => "usize",
        "double" => "double",
        "float" => "float",
        "bool" => "bool",
        _ => return None,
    })
}

// What the call reads an argument as, and the Rust expression that makes it (`k` is its place among the inputs).
fn param_expr(p: &Param, next: Option<&Param>, k: Option<usize>, sized: &mut bool) -> Result<String, String> {
    let ty = p.ty.trim();
    let name = &p.name;
    let optional = name.contains("Optional");
    let in_out = name.contains("InOut");
    let out_only = !in_out && (name.ends_with("Out") || name.contains("OutOptional"));
    let at = |k: Option<usize>| match k {
        Some(k) => format!("Some({k})"),
        None => "None".to_string(),
    };
    if p.callback {
        return Err("a callback".into());
    }
    let stars = ty.matches('*').count();
    let base = ty.trim_end_matches(['*', ' ']);
    let (constant, base) = match base.strip_prefix("const ") {
        Some(rest) => (true, rest.trim()),
        None => (false, base),
    };
    match (stars, constant, base) {
        // (GetTrackInfo's `track`: a number REAPER reads as a MediaTrack* when it isn't a track's index, so any other
        // number crashes it; GetTrack, GetTrackName and GetMediaTrackInfo_Value read the same by ref)
        (0, _, "INT_PTR") => Err("a number REAPER also reads as a pointer (INT_PTR)".into()),
        (0, _, b) => match scalar_kind(b) {
            Some(kind) => Ok(match kind {
                "int" => format!("c.int({}, {name:?}, {optional})? as _", at(k)),
                "uint" | "usize" | "isize" | "int64" => format!("c.int({}, {name:?}, {optional})? as _", at(k)),
                "double" => format!("c.double({}, {name:?}, {optional})?", at(k)),
                "float" => format!("c.double({}, {name:?}, {optional})? as f32", at(k)),
                _ => format!("c.boolean({}, {name:?}, {optional})?", at(k)),
            }),
            None => Err(format!("a {ty}")),
        },
        // (bytes with their size: REAPER reads that many from a JSON text that can hold no NUL, nor bytes past 0x7f as
        // themselves)
        (1, true, "char") if next.is_some_and(|n| n.ty.trim() == "int" && n.name == format!("{name}_sz")) => {
            Err(format!("binary data with its size ({name})"))
        }
        (1, true, "char") => Ok(format!("c.text({}, {name:?}, {optional})?", at(k))),
        (1, false, "char") => {
            let size = next.filter(|n| n.ty.trim() == "int" && n.name == format!("{name}_sz"));
            let input = in_out || name.contains("NeedBig") && !out_only;
            *sized = size.is_some();
            let k = if input { at(k) } else { "None".into() };
            // (a buffer REAPER isn't told the size of is never made smaller than the default, whatever the caller asks)
            Ok(format!("c.buf({k}, {name:?}, {sized})?", sized = size.is_some()))
        }
        (1, true, b) if scalar_kind(b).is_some() => Ok(format!("c.in_ptr_{}({}, {name:?})? as _", scalar_kind(b).unwrap(), at(k))),
        (1, false, b) if scalar_kind(b).is_some() => {
            let kind = scalar_kind(b).unwrap();
            if kind == "float" || kind == "isize" || kind == "usize" {
                return Err(format!("a {ty}"));
            }
            // (an array REAPER fills, as GetMediaItemTake_Peaks's buf: one value would be overrun)
            let lower = name.to_ascii_lowercase();
            if lower.contains("buf") || lower.contains("samples") || lower.contains("array") {
                return Err(format!("an array REAPER fills ({name})"));
            }
            let k = if out_only { "None".into() } else { at(k) };
            Ok(format!("c.out_{kind}({k}, {name:?})? as _"))
        }
        (2, true, "char") if out_only => Ok(format!("c.out_text({name:?})")),
        (1, false, b) if HANDLES.contains(&b) => Ok(format!("c.handle({}, {name:?}, {b:?}, {optional})? as *mut {b}", at(k))),
        _ => Err(format!("a {ty}")),
    }
}

// Whether a parameter is something the caller passes (an input), as opposed to what the call makes up itself.
fn is_input(p: &Param) -> bool {
    let ty = p.ty.trim();
    let name = &p.name;
    let in_out = name.contains("InOut");
    let out_only = !in_out && (name.ends_with("Out") || name.contains("OutOptional"));
    if ty == "char*" {
        return in_out || (name.contains("NeedBig") && !out_only);
    }
    if ty == "const char**" {
        return false;
    }
    if ty.ends_with('*') && !ty.starts_with("const") && scalar_kind(ty.trim_end_matches(['*', ' '])).is_some() {
        return !out_only;
    }
    true
}

fn ret_expr(ret: &str) -> Result<String, String> {
    let ty = ret.trim();
    let stars = ty.matches('*').count();
    let base = ty.trim_end_matches(['*', ' ']);
    let base = base.strip_prefix("const ").map(str::trim).unwrap_or(base);
    match (stars, base) {
        (0, "void") => Ok("Value::Null".into()),
        (0, b) if scalar_kind(b).is_some() => Ok(match scalar_kind(b).unwrap() {
            "float" => "Value::from(r as f64)".into(),
            "isize" => "Value::from(r as i64)".into(),
            "usize" => "Value::from(r as u64)".into(),
            "int" if b == "char" => "Value::from(r as i64)".into(),
            _ => "Value::from(r)".into(),
        }),
        (1, "char") => Ok("c.ret_text(r as *const c_char)".into()),
        (1, b) if HANDLES.contains(&b) => Ok(format!("c.ret_handle({b:?}, r as *mut c_void)")),
        _ => Err(format!("returns a {ty}")),
    }
}

fn emit_generic(functions: &[Function]) -> String {
    let mut out = String::new();
    let mut arms = String::new();
    let mut why = String::new();
    let mut inputs_table = String::new();
    for (index, f) in functions.iter().enumerate() {
        let mut exprs = Vec::new();
        let mut reason = ret_expr(&f.ret).err();
        let mut k = 0usize;
        let mut names = Vec::new();
        let mut skip = false;
        // A GetSet… function's values REAPER writes back (`…Out`) are also the values it sets (GetSet_LoopTimeRange's
        // start and end): taken from the caller, and needed, when its set flag is on.
        let set_flag = f
            .name
            .starts_with("GetSet")
            .then(|| f.params.iter().find(|p| p.ty.trim() == "bool" && SET_FLAGS.contains(&p.name.as_str())))
            .flatten();
        let mut set_k = None;
        for (i, p) in f.params.iter().enumerate() {
            if skip {
                skip = false;
                continue;
            }
            if set_flag.is_some_and(|flag| flag.name == p.name) {
                set_k = Some(k);
            }
            if let (Some(flag), Some(flag_k), "double*") = (set_flag, set_k, p.ty.trim()) {
                if out_only(&p.name) {
                    exprs.push(format!("c.out_double_set(Some({k}), {:?}, Some({flag_k}), {:?})?", p.name, flag.name));
                    names.push(format!("{:?}", p.name));
                    k += 1;
                    continue;
                }
            }
            let input = is_input(p);
            // A buffer whose size REAPER reads and writes back (`char* name` then `int* name_sz`): one expression that makes
            // both, the buffer as big as the size the caller gives (the size is an input; the buffer is one when REAPER reads it).
            if let Some(size) =
                f.params.get(i + 1).filter(|n| p.ty.trim() == "char*" && n.ty.trim() == "int*" && n.name == format!("{}_sz", p.name))
            {
                let at = |k: Option<usize>| k.map_or("None".to_string(), |k| format!("Some({k})"));
                let buf_k = input.then_some(k);
                if input {
                    names.push(format!("{:?}", p.name));
                    k += 1;
                }
                exprs.push(format!("c.buf_in_out({}, {:?}, {}, {:?})?", at(buf_k), p.name, at(Some(k)), size.name));
                names.push(format!("{:?}", size.name));
                k += 1;
                skip = true;
                continue;
            }
            let mut sized = false;
            match param_expr(p, f.params.get(i + 1), input.then_some(k), &mut sized) {
                Ok(expr) => {
                    if sized {
                        // (the buffer and its size: one call that answers both)
                        exprs.push(format!("{{ let (b, n) = {expr}; (b, n) }}"));
                        skip = true;
                    } else {
                        exprs.push(expr);
                    }
                }
                Err(e) if reason.is_none() => reason = Some(format!("{} takes {e}", p.name)),
                Err(_) => {}
            }
            if input {
                names.push(format!("{:?}", p.name));
                k += 1;
            }
        }
        writeln!(inputs_table, "    &[{}],", names.join(", ")).unwrap();
        match reason {
            Some(reason) => {
                writeln!(why, "    Some({reason:?}),").unwrap();
            }
            None => {
                writeln!(why, "    None,").unwrap();
                // A sized buffer is one expression that makes two arguments.
                let mut lets = String::new();
                let mut call_args = Vec::new();
                for (n, expr) in exprs.iter().enumerate() {
                    if expr.starts_with("{ let (b, n)") || expr.starts_with("c.buf_in_out(") {
                        writeln!(lets, "            let (a{n}, a{n}_sz) = {expr};").unwrap();
                        call_args.push(format!("a{n}"));
                        call_args.push(format!("a{n}_sz"));
                    } else if expr.starts_with("c.buf(") {
                        writeln!(lets, "            let (a{n}, _) = {expr};").unwrap();
                        call_args.push(format!("a{n}"));
                    } else {
                        writeln!(lets, "            let a{n} = {expr};").unwrap();
                        call_args.push(format!("a{n}"));
                    }
                }
                let ret = ret_expr(&f.ret).unwrap();
                writeln!(arms, "        {index} => {{").unwrap();
                writeln!(arms, "            let f = api.{}().ok_or_else(|| \"this REAPER has no {}\".to_string())?;", f.name, f.name)
                    .unwrap();
                arms.push_str(&lets);
                writeln!(arms, "            if c.dry() {{ return Ok(Value::Null); }}").unwrap();
                writeln!(arms, "            #[allow(clippy::let_unit_value, unused_variables)]").unwrap();
                writeln!(arms, "            let r = unsafe {{ f({}) }};", call_args.join(", ")).unwrap();
                writeln!(arms, "            Ok({ret})").unwrap();
                writeln!(arms, "        }}").unwrap();
            }
        }
    }
    writeln!(out, "// Generated by build.rs from sdk/reaper_plugin_functions.h: the generic call. Don't edit.").unwrap();
    writeln!(out, "/// Why each function can't be called by name (None: it can), in the table's order.").unwrap();
    writeln!(out, "pub static UNCALLABLE: [Option<&str>; {}] = [\n{why}];", functions.len()).unwrap();
    writeln!(out, "/// The arguments each function takes from the caller, by their C names, in order.").unwrap();
    writeln!(out, "pub static INPUTS: [&[&str]; {}] = [\n{inputs_table}];", functions.len()).unwrap();
    writeln!(out, "/// Calls function `index` of the table with the call's arguments.").unwrap();
    writeln!(out, "#[allow(unused_unsafe, clippy::unnecessary_cast, clippy::useless_conversion)]").unwrap();
    writeln!(out, "pub fn dispatch(api: &Api, index: usize, c: &mut Call) -> Result<Value, String> {{").unwrap();
    writeln!(out, "    match index {{\n{arms}        _ => Err(UNCALLABLE.get(index).copied().flatten().unwrap_or(\"no such function\").to_string()),\n    }}\n}}").unwrap();
    out
}
