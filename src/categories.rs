//! File categories by extension (port of Models/FileCategories.cs).

use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Category {
    Video,
    Images,
    Music,
    Documents,
    Archives,
    Programs,
    Code,
    Games,
    System,
    Other,
}

pub const COUNT: usize = 10;

pub struct Info {
    pub title: &'static str,
    /// 0xRRGGBB
    pub color: u32,
}

pub const ALL: [Info; COUNT] = [
    Info { title: "Видео", color: 0x8B5CF6 },
    Info { title: "Изображения", color: 0xEC4899 },
    Info { title: "Музыка", color: 0xF97316 },
    Info { title: "Документы", color: 0x3B82F6 },
    Info { title: "Архивы и образы", color: 0xD4A017 },
    Info { title: "Программы", color: 0x10B981 },
    Info { title: "Код и данные", color: 0x0EA5C6 },
    Info { title: "Игровые ресурсы", color: 0xE11D48 },
    Info { title: "Системные и служебные", color: 0x64748B },
    Info { title: "Прочее", color: 0x5B6B8C },
];

pub fn info(c: u8) -> &'static Info {
    &ALL[(c as usize).min(COUNT - 1)]
}

static MAP: LazyLock<HashMap<&'static str, Category>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    let mut add = |c: Category, list: &'static str| {
        for e in list.split_ascii_whitespace() {
            m.insert(e, c);
        }
    };
    add(Category::Video, "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts m2ts mts 3gp vob ogv rmvb divx");
    add(Category::Images, "jpg jpeg png gif bmp tif tiff webp heic heif raw cr2 cr3 nef arw dng orf psd psb svg ico avif jxl xcf kra ai eps");
    add(Category::Music, "mp3 flac wav aac ogg m4a wma opus aiff aif alac ape mid midi");
    add(Category::Documents, "pdf doc docx xls xlsx xlsm ppt pptx odt ods odp txt rtf csv md epub djvu djv fb2 mobi xps one pages key numbers vsdx");
    add(Category::Archives, "zip rar 7z tar gz tgz bz2 xz zst lz4 iso img cab vhd vhdx vmdk vdi wim esd dmg qcow2 apk");
    add(Category::Programs, "exe msi dll sys msix msixbundle appx appxbundle ocx com scr drv cpl bat cmd ps1 jar node so");
    add(Category::Code, "cs js mjs cjs ts tsx jsx py pyc java class cpp cc c h hpp go rs php rb json xml html htm css scss less vue kt swift sql yml yaml toml lua pdb obj lib a o ipynb wasm map lock gradle sln csproj");
    add(Category::Games, "pak vpk upk uasset uexp ubulk utoc ucas bsa ba2 forge rpf wad big cpk xnb pck bundle assets resource sav esm esp gcf ncf vtf");
    add(Category::System, "tmp temp log etl evtx dmp mdmp bak old db sqlite sqlite3 ldb cache blf regtrans-ms edb chk pf mui cat manifest mum dat bin cab_ swp lck lock- ini cfg");
    m
});

const SYSTEM_FILES: [&str; 6] = [
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "dumpstack.log",
    "dumpstack.log.tmp",
    "memory.dmp",
];

pub fn of(file_name: &str) -> Category {
    if file_name.len() <= 18 {
        let lower = file_name.to_ascii_lowercase();
        if SYSTEM_FILES.contains(&lower.as_str()) {
            return Category::System;
        }
    }
    let Some(dot) = file_name.rfind('.') else { return Category::Other };
    if dot == 0 || dot == file_name.len() - 1 || file_name.len() - dot > 16 {
        return Category::Other;
    }
    let ext = &file_name[dot + 1..];
    let mut buf = [0u8; 16];
    let bytes = ext.as_bytes();
    if bytes.len() > buf.len() {
        return Category::Other;
    }
    for (b, s) in buf.iter_mut().zip(bytes) {
        *b = s.to_ascii_lowercase();
    }
    match std::str::from_utf8(&buf[..bytes.len()]) {
        Ok(lower) => MAP.get(lower).copied().unwrap_or(Category::Other),
        Err(_) => Category::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(of("movie.MKV"), Category::Video);
        assert_eq!(of("pagefile.sys"), Category::System);
        assert_eq!(of("driver.sys"), Category::Programs);
        assert_eq!(of(".gitignore"), Category::Other);
        assert_eq!(of("noext"), Category::Other);
        assert_eq!(of("архив.7z"), Category::Archives);
    }
}
