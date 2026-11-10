//! 结构体布局、枚举值和类型清单（ghostty_type_json）一致，同 mygo 的 types.go 核对。

use std::ffi::CStr;

use serde_json::Value;

use super::lib;
use crate::{ffi, Key, MOUSE_SHAPES};

fn manifest() -> Value {
    // SAFETY：ghostty_type_json 返回静态的、以 NUL 结尾的字符串。
    let json = unsafe { CStr::from_ptr((ffi::api().type_json)()) };
    serde_json::from_str(json.to_str().unwrap()).unwrap()
}

#[test]
fn structs_match_the_manifest() {
    if !lib() {
        return;
    }
    let m = manifest();
    let types = &m["types"];
    assert_eq!(
        m["abi"]["pointer_size"].as_u64(),
        Some(std::mem::size_of::<usize>() as u64)
    );
    let layouts = ffi::layouts();
    assert!(layouts.len() >= 30, "只核对了 {} 个结构体", layouts.len());
    for (name, size, fields) in layouts {
        let t = &types[name];
        assert_eq!(t["size"].as_u64(), Some(size as u64), "{name} 的大小");
        for (field, off) in fields {
            assert_eq!(
                t["fields"][field]["offset"].as_u64(),
                Some(off as u64),
                "{name}.{field} 的偏移"
            );
        }
    }
}

#[test]
fn enums_match_the_manifest() {
    if !lib() {
        return;
    }
    let m = manifest();
    let types = &m["types"];
    for &(ty, name, value) in ffi::ENUMS {
        assert_eq!(
            types[ty]["values"][name].as_i64(),
            Some(value),
            "{ty} {name}"
        );
    }
    // 每个键：名字和值都对得上，个数也一样（多出来的只有 MAX_VALUE）。
    let keys = types["GhosttyKey"]["values"].as_object().unwrap();
    assert_eq!(keys.len(), Key::ALL.len() + 1);
    for &k in Key::ALL {
        assert_eq!(
            keys.get(&k.manifest_name()).and_then(Value::as_i64),
            Some(k as i64),
            "GhosttyKey {}",
            k.manifest_name()
        );
        assert_eq!(Key::from_code(k.code()), k);
    }
    // OSC 22 的指针形状按 Ghostty 的顺序。
    let shapes = &types["GhosttyMouseShape"]["values"];
    for (i, name) in MOUSE_SHAPES.iter().enumerate() {
        let name = name.to_ascii_uppercase().replace('-', "_");
        assert_eq!(
            shapes[name.as_str()].as_i64(),
            Some(i as i64),
            "指针形状 {name}"
        );
    }
}

#[test]
fn load_checks_the_manifest() {
    if !lib() {
        return;
    }
    // SAFETY：同上。
    let json = unsafe { CStr::from_ptr((ffi::api().type_json)()) };
    let json = json.to_str().unwrap();
    assert!(ffi::check_manifest(json).is_ok());
    // 布局、枚举对不上的库不载入。
    let mut bad: Value = serde_json::from_str(json).unwrap();
    bad["types"]["GhosttyGridRef"]["fields"]["x"]["offset"] = 20.into();
    assert!(ffi::check_manifest(&bad.to_string()).is_err());
    let mut bad: Value = serde_json::from_str(json).unwrap();
    bad["types"]["GhosttyKey"]["values"]["TAB"] = 1000.into();
    assert!(ffi::check_manifest(&bad.to_string()).is_err());
    assert!(crate::loaded_path().unwrap().is_file());
}
