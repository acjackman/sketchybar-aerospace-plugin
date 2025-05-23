use std::env;
use std::fs;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::io::prelude::*;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::ffi::{CStr,c_char};
use std::path::PathBuf;

fn get_socket_path() -> PathBuf {
    let username = std::env::var("USER").unwrap_or_else(|_| String::from("unknown"));
    PathBuf::from("/tmp").join(format!("bobko.aerospace-{}.sock", username))
}

static BUF_SIZE: usize = 8196;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AerospaceResponse {
    stderr: String,
    exit_code: u32,
    stdout: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppFontEntry {
    icon_name: String,
    app_names: Vec<String>
}

#[derive(Debug, Deserialize)]
struct SketchybarDisplayFrame {
    x: f32,
}

#[derive(Debug ,Deserialize)]
struct SketchybarDisplay {
    #[serde(alias = "arrangement-id")]
    arrangement_id: i32,
    frame: SketchybarDisplayFrame,
}

#[derive(Debug, Deserialize)]
struct SketchybarBar {
    items: Vec<String>
}

#[derive(Debug, Deserialize)]
struct AerospaceWorkspace {
    #[serde(alias = "monitor-id")]
    monitor_id: i32,
    workspace: String,
    #[serde(alias = "workspace-is-focused")]
    focused: bool,
    #[serde(alias = "workspace-is-visible")]
    visible: bool,
}

#[derive(Debug, Deserialize)]
struct AerospaceWindow {
    #[serde(alias = "app-name")]
    app_name: String,
    #[serde(alias = "window-title")]
    window_title: String,
    #[serde(alias = "workspace")]
    workspace: String,
}

fn app_font_map(path: &str) -> std::io::Result<HashMap<String, String>> {
    let app_font_str = fs::read_to_string(path)
        .expect(&format!("App font json should be available at {}", path));

    let app_font_data: Vec<AppFontEntry> = match serde_json::from_str(&app_font_str) {
        Ok(data) => data,
        Err(e) => {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Failed to parse JSON from {}: {}", path, e)));
        }
    };
    let mut result: HashMap<String, String> = HashMap::new();

    for entry in app_font_data {
        for name in entry.app_names {
            result.insert(name, entry.icon_name.clone());
        }
    }

    if result.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "App font map is empty"));
    }

    Ok(result)
}

unsafe extern "C" {
    fn sketchybar_call(message: *const c_char, message_length: usize) -> *const c_char;
}

fn _sketchybar_call(message_bytes: &Vec<i8>) -> std::io::Result<&str> {
    let char_ptr = unsafe { sketchybar_call(message_bytes.as_ptr(), message_bytes.len()) };
    let c_str = unsafe { CStr::from_ptr(char_ptr) };
    Ok(c_str.to_str().unwrap())
}

fn sketchybar_query<T: DeserializeOwned>(message: &str) -> std::io::Result<T> {
    let message_fmt = format!("--query {message}");
    let message_bytes: Vec<i8> = message_fmt.bytes().map(|c| {
        if c == b' ' {
            0
        } else {
            c as i8
        }
    }).collect();
    let resp_value: T = serde_json::from_str(_sketchybar_call(&message_bytes)?)?;
    Ok(resp_value)
}

fn sketchybar_batched(messages: &Vec<Vec<i8>>) -> Result<(), &str> {
    let message_bytes: Vec<i8> = messages.join(&0);

    let char_ptr = unsafe { sketchybar_call(message_bytes.as_ptr(), message_bytes.len()) };
    let c_str = unsafe { CStr::from_ptr(char_ptr) };
    let ret_str = c_str.to_str().unwrap();
    if !ret_str.is_empty() {
        println!("{}", c_str.to_str().unwrap());
        return Err(ret_str);
    }
    Ok(())
}

fn sketchybar_set(entry_name: &str, params: Value) -> Result<Vec<i8>, &str> {
    let mut message_bytes: Vec<i8> = b"--set".map(|c| c as i8).to_vec();
    message_bytes.push(0);
    message_bytes.extend(entry_name.bytes().map(|c| c as i8));
    let mut objects = vec![(String::new(), params.as_object().unwrap())];
    loop {
        if objects.is_empty() {
            break;
        }
        let (prefix, obj) = objects.pop().unwrap();
        for (k, v) in obj {
            let prefixed_key = format!("{prefix}{k}");
            if v.is_object() {
                objects.push((format!("{prefixed_key}."), v.as_object().unwrap()));
            } else {
                message_bytes.push(0);
                message_bytes.extend(prefixed_key.bytes().map(|c| c as i8));
                message_bytes.push(b'=' as i8);
                let v_str = if let Some(val) = v.as_bool() {
                    if val { String::from("on") } else { String::from("off") }
                } else if let Some(val) = v.as_i64() {
                    val.to_string()
                } else if let Some(val) = v.as_f64() {
                    val.to_string()
                } else if let Some(val) = v.as_str() {
                    val.to_string()
                } else {
                    return Err("Failed to convert");
                };
                message_bytes.extend(v_str.bytes().map(|c| c as i8));
            }
        }
    }
    Ok(message_bytes)
}

fn sketchybar_add<'a,'b>(entry_type: &'a str, entry_name: &'a str, entry_position: &'a str) -> Result<Vec<i8>, &'b str> {
    let mut message_bytes: Vec<i8> = b"--add".map(|c| c as i8).to_vec();
    message_bytes.push(0);
    message_bytes.extend(entry_type.bytes().map(|c| c as i8));
    message_bytes.push(0);
    message_bytes.extend(entry_name.bytes().map(|c| c as i8));
    message_bytes.push(0);
    message_bytes.extend(entry_position.bytes().map(|c| c as i8));
    Ok(message_bytes)
}

fn sketchybar_remove<'a,'b>(entry_name: &'a str) -> Result<Vec<i8>, &'b str> {
    let mut message_bytes: Vec<i8> = b"--remove".map(|c| c as i8).to_vec();
    message_bytes.push(0);
    message_bytes.extend(entry_name.bytes().map(|c| c as i8));
    Ok(message_bytes)
}

fn aerospace_command<T: DeserializeOwned>(stream: &mut UnixStream, command: &str) -> std::io::Result<T> {
    let j = json!({
        "args": format!("{command} --json").split(" ").collect::<Vec<&str>>(),
        "command": "",
        "stdin": "",
    });
    stream.write_all(j.to_string().as_bytes())?;
    let mut buf = [0; BUF_SIZE];
    let count = stream.read(&mut buf)?;
    let response = String::from_utf8(buf[..count].to_vec()).unwrap();
    let resp_data: AerospaceResponse = serde_json::from_str(&response)?;
    if resp_data.exit_code > 0 {
        return Err(std::io::Error::other(resp_data.stderr));
    }
    let resp_payload: T = serde_json::from_str(&resp_data.stdout)?;
    Ok(resp_payload)
}

fn main() -> std::io::Result<()> {

    // quickly switch background if we have it
    match (std::env::var("FOCUSED"), std::env::var("PREV_FOCUSED")) {
        (Ok(val), Ok(prev_val)) => {
            let mut env_msgs: Vec<Vec<i8>> = Vec::new();
            if let Ok(msg) = sketchybar_set(&format!("space.{val}"), json!(
                    {
                        "background": {"color": "0x44ffffff"},
                        "label": {"color": "0xffffffff"},
                        "icon": {"color": "0xffffffff"},
                    })) {
                env_msgs.push(msg);
            }
            if let Ok(msg) = sketchybar_set(&format!("space.{prev_val}"), json!(
                    {
                        "background": {"color": "0x00000000"},
                        "label": {"color": "0xffa0a0a0"},
                        "icon": {"color": "0xffa0a0a0"},
                    })) {
                env_msgs.push(msg);
            }
            if !env_msgs.is_empty() {
                let _ = sketchybar_batched(&env_msgs);
            }
            return Ok(());
        },
        (_, _) => {}
    }

    let socket_path = get_socket_path();
    let mut stream = UnixStream::connect(socket_path)?;

    let mut displays: Vec<SketchybarDisplay> = sketchybar_query("displays").unwrap();
    displays.sort_by(|a, b| a.frame.x.total_cmp(&b.frame.x));
    let bar_props: SketchybarBar = sketchybar_query("bar").unwrap();
    let mut items_exist: HashMap<String, bool> = bar_props.items.iter().filter(|n| n.contains("space.")).map(|n| (n.clone(), false)).collect();

    let app_to_font = match app_font_map(&format!("{}/.config/sketchybar/data/icon_map.json", env::var("HOME").unwrap_or_else(|_| String::from("/tmp")))) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("Failed to load icon map: {}", e);
            HashMap::new()
        }
    };

    let workspaces: Vec<AerospaceWorkspace> = aerospace_command(&mut stream, "list-workspaces --all --format %{monitor-id}%{workspace}%{workspace-is-visible}%{workspace-is-focused}")?;
    let windows: Vec<AerospaceWindow> = aerospace_command(&mut stream, "list-windows --all --format %{app-name}%{window-title}%{workspace}")?;

    let mut messages: Vec<Vec<i8>> = Vec::new();
    for workspace in workspaces {
        let space = workspace.workspace;
        let space_name = format!("space.{space}");

        
        // Skip numerical workspaces with no windows
        let is_numerical = space.parse::<i32>().is_ok();
        let window_count = windows.iter().filter(|w| w.window_title != "" && w.workspace == space).count();
        if is_numerical && window_count == 0 {
            if let Some(e) = items_exist.get_mut(&space_name) {
                *e = true;
                if let Ok(m) = sketchybar_remove(&space_name) {
                    messages.push(m);
                }
            }
            continue;
        }

        let mut cur_apps: Vec<String> = windows.iter().enumerate().filter(|(_, w)| {
            w.window_title != "" && w.workspace == space
        }).map(|(_, w)| {
            if let Some(n) = app_to_font.get(w.app_name.as_str()) {
                n.clone()
            } else {
                String::from(":chevron_right:")
            }
        }).collect();
        cur_apps.sort();
        cur_apps.dedup();
        let label = String::from(" ") + &cur_apps.join(" ");

        let display_id = displays[(workspace.monitor_id - 1) as usize].arrangement_id;

        let rpad = if label.len() < 2 { 0 } else { 10 };
        let bg_color= if workspace.focused { "0x44ffffff" } else { "0x00000000" };
        let color = if workspace.visible { "0xffffffff" } else { "0xffa0a0a0" };
        let params = json!({
            "background": {
                "color": bg_color,
                "corner_radius": 5,
                "height": 20,
            },
            "label": {
                "string": label,
                "color": color,
                "padding_right": rpad,
                "padding_left": 6,
                "y_offset": -1,
                "drawing": "on",
                "font": "sketchybar-app-font:Mono:12.0",
            },
            "icon": {
                "string": space,
                "font": {
                    "size": 12.0
                },
                "padding_left": 8,
                "padding_right": 0,
                "color": color,
            },
            "click_script": format!("aerospace workspace {space}"),
            "display": display_id,
        });

        if let Some(e) = items_exist.get_mut(&space_name) {
            *e = true;
            if let Ok(m) = sketchybar_set(&space_name, params) {
                messages.push(m);
            }
        } else {
            if let Ok(m) = sketchybar_add("item", &space_name, "left") {
                messages.push(m);
            }
            if let Ok(m) = sketchybar_set(&space_name, params) {
                messages.push(m);
            }
        }
    }

    for (n, e) in items_exist {
        if !e {
            if let Ok(m) = sketchybar_remove(&n) {
                messages.push(m);
            }
        }
    }
    // let messages_fmt = messages.iter().map(|m| String::from_utf8(
    //         m.iter().map(
    //             |c| if *c == 0 { '|' as u8 } else { *c as u8 }
    //             ).collect::<Vec<u8>>()).unwrap()).collect::<Vec<String>>().join("#");
    // println!("{}", messages_fmt);
    let _ = sketchybar_batched(&messages);
    Ok(())
}
