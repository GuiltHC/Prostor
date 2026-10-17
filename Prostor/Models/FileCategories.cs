using System.Windows.Media;

namespace Prostor.Models;

public enum FileCategory : byte
{
    Video, Images, Music, Documents, Archives, Programs, Code, Games, System, Other
}

public sealed class CategoryInfo
{
    public FileCategory Category { get; init; }
    public string Title { get; init; }
    public string Glyph { get; init; }
    public Color Color { get; init; }
    public SolidColorBrush Brush { get; init; }
}

public static class FileCategories
{
    public static readonly CategoryInfo[] All =
    {
        Make(FileCategory.Video, "Видео", "", "#8B5CF6"),
        Make(FileCategory.Images, "Изображения", "", "#EC4899"),
        Make(FileCategory.Music, "Музыка", "", "#F97316"),
        Make(FileCategory.Documents, "Документы", "", "#3B82F6"),
        Make(FileCategory.Archives, "Архивы и образы", "", "#D4A017"),
        Make(FileCategory.Programs, "Программы", "", "#10B981"),
        Make(FileCategory.Code, "Код и данные", "", "#0EA5C6"),
        Make(FileCategory.Games, "Игровые ресурсы", "", "#E11D48"),
        Make(FileCategory.System, "Системные и служебные", "", "#64748B"),
        Make(FileCategory.Other, "Прочее", "", "#5B6B8C"),
    };

    public static CategoryInfo Get(FileCategory c) => All[(int)c];
    public static CategoryInfo Get(byte c) => All[c < All.Length ? c : (int)FileCategory.Other];

    private static CategoryInfo Make(FileCategory c, string title, string glyph, string hex)
    {
        var color = (Color)ColorConverter.ConvertFromString(hex);
        var brush = new SolidColorBrush(color);
        brush.Freeze();
        return new CategoryInfo { Category = c, Title = title, Glyph = glyph, Color = color, Brush = brush };
    }

    private static readonly Dictionary<string, FileCategory> Map = Build();
    private static readonly Dictionary<string, FileCategory>.AlternateLookup<ReadOnlySpan<char>> Lookup =
        Map.GetAlternateLookup<ReadOnlySpan<char>>();

    private static Dictionary<string, FileCategory> Build()
    {
        var m = new Dictionary<string, FileCategory>(StringComparer.OrdinalIgnoreCase);
        void Add(FileCategory c, string list)
        {
            foreach (var e in list.Split(' ', StringSplitOptions.RemoveEmptyEntries)) m[e] = c;
        }
        Add(FileCategory.Video, "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts m2ts mts 3gp vob ogv rmvb divx");
        Add(FileCategory.Images, "jpg jpeg png gif bmp tif tiff webp heic heif raw cr2 cr3 nef arw dng orf psd psb svg ico avif jxl xcf kra ai eps");
        Add(FileCategory.Music, "mp3 flac wav aac ogg m4a wma opus aiff aif alac ape mid midi");
        Add(FileCategory.Documents, "pdf doc docx xls xlsx xlsm ppt pptx odt ods odp txt rtf csv md epub djvu djv fb2 mobi xps one pages key numbers vsdx");
        Add(FileCategory.Archives, "zip rar 7z tar gz tgz bz2 xz zst lz4 iso img cab vhd vhdx vmdk vdi wim esd dmg qcow2 apk");
        Add(FileCategory.Programs, "exe msi dll sys msix msixbundle appx appxbundle ocx com scr drv cpl bat cmd ps1 jar node so");
        Add(FileCategory.Code, "cs js mjs cjs ts tsx jsx py pyc java class cpp cc c h hpp go rs php rb json xml html htm css scss less vue kt swift sql yml yaml toml lua pdb obj lib a o ipynb wasm map lock gradle sln csproj");
        Add(FileCategory.Games, "pak vpk upk uasset uexp ubulk utoc ucas bsa ba2 forge rpf wad big cpk xnb pck bundle assets resource sav esm esp gcf ncf vtf");
        Add(FileCategory.System, "tmp temp log etl evtx dmp mdmp bak old db sqlite sqlite3 ldb cache blf regtrans-ms edb chk pf mui cat manifest mum dat bin cab_ swp lck lock- ini cfg");
        return m;
    }

    private static readonly HashSet<string> SystemFiles = new(StringComparer.OrdinalIgnoreCase)
    {
        "pagefile.sys", "hiberfil.sys", "swapfile.sys", "DumpStack.log", "DumpStack.log.tmp", "MEMORY.DMP"
    };

    public static FileCategory Of(string fileName)
    {
        if (fileName.Length <= 18 && SystemFiles.Contains(fileName)) return FileCategory.System;
        ReadOnlySpan<char> span = fileName;
        int dot = span.LastIndexOf('.');
        if (dot <= 0 || dot == span.Length - 1) return FileCategory.Other;
        var ext = span.Slice(dot + 1);
        return Lookup.TryGetValue(ext, out var c) ? c : FileCategory.Other;
    }

    public static ReadOnlySpan<char> Extension(string fileName)
    {
        ReadOnlySpan<char> span = fileName;
        int dot = span.LastIndexOf('.');
        if (dot <= 0 || dot == span.Length - 1 || span.Length - dot > 16) return ReadOnlySpan<char>.Empty;
        return span.Slice(dot + 1);
    }
}
