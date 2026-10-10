use nuzky_engine::edit::CaptionPreset;
use nuzky_mcp::caption_styles;

use crate::{CmdResult, err};

#[tauri::command]
pub fn caption_styles() -> CmdResult<Vec<CaptionPreset>> {
    caption_styles::list().map_err(err)
}

#[tauri::command]
pub fn save_caption_style(style: CaptionPreset) -> CmdResult<Vec<CaptionPreset>> {
    caption_styles::save(style).map_err(err)
}

#[tauri::command]
pub fn rename_caption_style(from: String, to: String) -> CmdResult<Vec<CaptionPreset>> {
    caption_styles::rename(&from, &to).map_err(err)
}

#[tauri::command]
pub fn delete_caption_style(name: String) -> CmdResult<Vec<CaptionPreset>> {
    caption_styles::delete(&name).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nuzky_engine::edit::{caption_preset, new_id};

    #[test]
    fn caption_styles_round_trip() {
        if std::env::var_os("NUZKY_CAPTION_STYLES_TEST").is_none() {
            let dir = std::env::temp_dir().join(format!("nuzky-caption-styles-{}", new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "caption_styles::tests::caption_styles_round_trip", "--nocapture"])
                .env("NUZKY_CAPTION_STYLES_TEST", "1")
                .env("XDG_DATA_HOME", &dir)
                .env("HOME", &dir)
                .status()
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
            assert!(status.success());
            return;
        }
        assert!(caption_styles().unwrap().is_empty());
        let mut style = caption_preset("hormozi").unwrap().clone();
        style.name = "  My style  ".into();
        let saved = save_caption_style(style.clone()).unwrap();
        style.name = "My style".into();
        assert_eq!(saved, vec![style.clone()]);
        assert_eq!(caption_styles().unwrap(), saved);
        for name in ["MY STYLE", "my_style", "my-style", "REEL", "Green_Box", "Hormozi-green"] {
            let mut duplicate = style.clone();
            duplicate.name = name.into();
            assert!(save_caption_style(duplicate).unwrap_err().starts_with("NAME_TAKEN"));
        }
        for name in [" ".into(), "x".repeat(41)] {
            let mut invalid = style.clone();
            invalid.name = name;
            assert!(save_caption_style(invalid).unwrap_err().starts_with("INVALID_ARGUMENTS"));
        }
        let mut invalid = style.clone();
        invalid.name = "Invalid".into();
        invalid.style.keywords.as_mut().unwrap().color = "#€".into();
        assert!(save_caption_style(invalid).unwrap_err().starts_with("INVALID_ARGUMENTS"));
        for duration in [-1, 10_000_001] {
            let mut invalid = style.clone();
            invalid.name = "Invalid".into();
            invalid.anim_in.as_mut().unwrap().duration_us = duration;
            assert!(save_caption_style(invalid).unwrap_err().starts_with("INVALID_ARGUMENTS"));
        }
        let renamed = rename_caption_style("MY_STYLE".into(), " New style ".into()).unwrap();
        assert_eq!(renamed[0].name, "New style");
        assert_eq!(caption_styles().unwrap(), renamed);
        assert!(rename_caption_style("New style".into(), "reel".into()).unwrap_err().starts_with("NAME_TAKEN"));
        assert!(delete_caption_style("missing".into()).unwrap_err().starts_with("UNKNOWN_STYLE"));
        assert!(rename_caption_style("missing".into(), "Other".into()).unwrap_err().starts_with("UNKNOWN_STYLE"));
        assert!(delete_caption_style("new-style".into()).unwrap().is_empty());
        assert!(caption_styles().unwrap().is_empty());
        let mut boundary = style.clone();
        boundary.name = "č".repeat(40);
        boundary.anim_in.as_mut().unwrap().duration_us = 0;
        boundary.anim_out = boundary.anim_in;
        boundary.anim_out.as_mut().unwrap().duration_us = 10_000_000;
        assert_eq!(save_caption_style(boundary.clone()).unwrap(), vec![boundary.clone()]);
        assert!(delete_caption_style(boundary.name).unwrap().is_empty());
        let path = dirs::data_dir().unwrap().join("nuzky/caption-styles.json");
        std::fs::write(&path, b"broken").unwrap();
        assert!(caption_styles().unwrap_err().starts_with("STORE_UNREADABLE"));
        assert!(save_caption_style(style.clone()).unwrap_err().starts_with("STORE_UNREADABLE"));
        assert_eq!(caption_styles::find("reel").unwrap(), *caption_preset("reel").unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(caption_styles().unwrap_err().starts_with("STORE_UNREADABLE"));
        assert!(save_caption_style(style).unwrap_err().starts_with("STORE_UNREADABLE"));
        assert!(path.is_dir());
    }
}
