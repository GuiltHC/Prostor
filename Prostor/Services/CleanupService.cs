using System.IO.Enumeration;
using Microsoft.Win32;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>Known junk locations: temporary files and caches that applications re-create on demand.</summary>
public static class CleanupService
{
    public const string GroupSystem = "Windows";
    public const string GroupBrowsers = "Браузеры";
    public const string GroupApps = "Приложения";
    public const string GroupDev = "Для разработчиков";
    public const string GroupDeep = "Глубокая очистка Windows";

    private static string Local => Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
    private static string Roaming => Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
    private static string ProgramData => Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData);
    private static string WinDir => Environment.GetFolderPath(Environment.SpecialFolder.Windows);
    private static string Profile => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

    private static readonly string[] ChromiumProfileCaches = { "Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "Media Cache" };
    private static readonly string[] ChromiumRootCaches = { "GrShaderCache", "ShaderCache", "GraphiteDawnCache", "Crashpad\\reports" };

    public static List<CleanupItem> BuildItems()
    {
        var items = new List<CleanupItem>();
        var oneDay = TimeSpan.FromDays(1);

        // ---------------- Windows
        items.Add(new CleanupItem
        {
            Id = "recycle", Group = GroupSystem, Glyph = "\uE74D", IsRecycleBin = true, IsChecked = true,
            Title = "Корзина",
            Description = "Файлы, которые вы уже удалили. После очистки их нельзя будет восстановить."
        });
        items.Add(Folder("temp-user", GroupSystem, "\uE7C3", "Временные файлы",
            "Папка TEMP вашей учётной записи. Удаляются только файлы старше суток.", true, false,
            new CleanTarget { Path = UserTempPath(), MinAge = oneDay }));
        items.Add(Folder("temp-win", GroupSystem, "\uE7C3", "Временные файлы Windows",
            "Системная папка Windows\\Temp. Удаляются только файлы старше суток.", true, true,
            new CleanTarget { Path = Path.Combine(WinDir, "Temp"), MinAge = oneDay }));
        items.Add(Folder("thumbs", GroupSystem, "\uEB9F", "Кэш эскизов",
            "Миниатюры картинок и видео в Проводнике. Windows создаст их заново.", true, false,
            new CleanTarget { Path = Path.Combine(Local, @"Microsoft\Windows\Explorer"), Pattern = "thumbcache_*.db", Recursive = false },
            new CleanTarget { Path = Path.Combine(Local, @"Microsoft\Windows\Explorer"), Pattern = "iconcache_*.db", Recursive = false }));
        items.Add(Folder("wu", GroupSystem, "\uE896", "Загрузки обновлений Windows",
            "Уже установленные пакеты обновлений из SoftwareDistribution\\Download.", true, true,
            new CleanTarget { Path = Path.Combine(WinDir, @"SoftwareDistribution\Download"), MinAge = TimeSpan.FromDays(3) }));
        items.Add(Folder("do", GroupSystem, "\uE8CE", "Кэш оптимизации доставки",
            "Фрагменты обновлений, которыми Windows делится с другими компьютерами.", true, true,
            new CleanTarget { Path = Path.Combine(WinDir, @"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache") }));
        items.Add(Folder("wer", GroupSystem, "\uEBE8", "Отчёты об ошибках",
            "Отчёты Windows об ошибках программ, которые уже не нужны.", true, false,
            new CleanTarget { Path = Path.Combine(Local, @"Microsoft\Windows\WER") },
            new CleanTarget { Path = Path.Combine(ProgramData, @"Microsoft\Windows\WER\ReportArchive") },
            new CleanTarget { Path = Path.Combine(ProgramData, @"Microsoft\Windows\WER\ReportQueue") }));
        items.Add(Folder("dumps", GroupSystem, "\uEBE8", "Дампы памяти",
            "Снимки памяти при сбоях программ и системы. Нужны только разработчикам для отладки.", true, false,
            new CleanTarget { Path = Path.Combine(Local, "CrashDumps") },
            new CleanTarget { Path = Path.Combine(WinDir, "Minidump") },
            new CleanTarget { Path = Path.Combine(WinDir, "MEMORY.DMP"), IsFile = true },
            new CleanTarget { Path = Path.Combine(WinDir, "LiveKernelReports"), Pattern = "*.dmp" }));
        items.Add(Folder("shaders", GroupSystem, "\uE950", "Кэш шейдеров видеокарты",
            "Кэш DirectX и драйверов NVIDIA / AMD / Intel. Игры пересоберут его сами, но первые запуски будут дольше.", false, false,
            new CleanTarget { Path = Path.Combine(Local, "D3DSCache") },
            new CleanTarget { Path = Path.Combine(Local, @"NVIDIA\DXCache") },
            new CleanTarget { Path = Path.Combine(Local, @"NVIDIA\GLCache") },
            new CleanTarget { Path = Path.Combine(Local, @"NVIDIA Corporation\NV_Cache") },
            new CleanTarget { Path = Path.Combine(Local, @"AMD\DxCache") },
            new CleanTarget { Path = Path.Combine(Local, @"AMD\DxcCache") },
            new CleanTarget { Path = Path.Combine(Local, @"AMD\GLCache") },
            new CleanTarget { Path = Path.Combine(Local, @"AMD\VkCache") },
            new CleanTarget { Path = Path.Combine(Local, @"Intel\ShaderCache") }));

        // ---------------- Deep system cleanup (administrator)
        string root = Path.GetPathRoot(WinDir) ?? "C:\\";
        string programFiles = Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles);
        items.Add(Folder("gpu-installers", GroupDeep, "\uE7F4", "Установщики драйверов видеокарты",
            "Распакованные и скачанные установщики NVIDIA и AMD. Драйверы уже установлены — эти копии не нужны.", true, true,
            new CleanTarget { Path = Path.Combine(root, "NVIDIA") },
            new CleanTarget { Path = Path.Combine(root, "AMD") },
            new CleanTarget { Path = Path.Combine(ProgramData, @"NVIDIA Corporation\Downloader") },
            new CleanTarget { Path = Path.Combine(programFiles, @"NVIDIA Corporation\Installer2") }));
        items.Add(Folder("winlogs", GroupDeep, "\uE9F9", "Журналы обслуживания Windows",
            "Логи установки обновлений (CBS, DISM). Нужны только для разбора ошибок.", true, true,
            new CleanTarget { Path = Path.Combine(WinDir, @"Logs\CBS"), MinAge = oneDay },
            new CleanTarget { Path = Path.Combine(WinDir, @"Logs\DISM"), MinAge = oneDay }));
        items.Add(Folder("patchcache", GroupDeep, "\uE8F1", "Кэш исправлений установщика",
            "Копии файлов, которые установщик Windows хранит для будущих исправлений программ ($PatchCache$).", false, true,
            new CleanTarget { Path = Path.Combine(WinDir, @"Installer\$PatchCache$") }));
        items.AddRange(DeepCleanup.Build(GroupDeep));

        // ---------------- Browsers
        AddChromium(items, "chrome", "Google Chrome", Path.Combine(Local, @"Google\Chrome\User Data"));
        AddChromium(items, "edge", "Microsoft Edge", Path.Combine(Local, @"Microsoft\Edge\User Data"));
        AddChromium(items, "yandex", "Яндекс Браузер", Path.Combine(Local, @"Yandex\YandexBrowser\User Data"));
        AddChromium(items, "opera", "Opera", Path.Combine(Local, @"Opera Software\Opera Stable"), Path.Combine(Roaming, @"Opera Software\Opera Stable"));
        AddChromium(items, "operagx", "Opera GX", Path.Combine(Local, @"Opera Software\Opera GX Stable"), Path.Combine(Roaming, @"Opera Software\Opera GX Stable"));
        AddChromium(items, "brave", "Brave", Path.Combine(Local, @"BraveSoftware\Brave-Browser\User Data"));
        AddChromium(items, "vivaldi", "Vivaldi", Path.Combine(Local, @"Vivaldi\User Data"));
        AddFirefox(items);

        // ---------------- Applications
        items.Add(Folder("discord", GroupApps, "\uE8BD", "Discord — кэш",
            "Кэш картинок и интерфейса Discord. Сообщения и настройки не затрагиваются.", true, false,
            ElectronCaches(Path.Combine(Roaming, "discord"))));
        items.Add(Folder("telegram", GroupApps, "\uE715", "Telegram — медиакэш",
            "Скачанные фото и видео из чатов. Telegram загрузит их снова при просмотре.", false, false,
            new CleanTarget { Path = Path.Combine(Roaming, @"Telegram Desktop\tdata\user_data") },
            new CleanTarget { Path = Path.Combine(Roaming, @"Telegram Desktop\tdata\user_data#2") },
            new CleanTarget { Path = Path.Combine(Roaming, @"Telegram Desktop\tdata\user_data#3") }));
        items.Add(Folder("steam", GroupApps, "\uE7FC", "Steam — веб-кэш",
            "Кэш встроенного браузера Steam. Игры и сохранения не затрагиваются.", true, false,
            new CleanTarget { Path = Path.Combine(Local, @"Steam\htmlcache") }));
        items.Add(Folder("spotify", GroupApps, "\uEC4F", "Spotify — кэш",
            "Временно сохранённые треки. Скачанные для офлайна плейлисты не удаляются.", true, false,
            new CleanTarget { Path = Path.Combine(Local, @"Spotify\Data") },
            new CleanTarget { Path = Path.Combine(Local, @"Spotify\Browser\Cache") }));
        items.Add(Folder("vscode", GroupApps, "\uE943", "Visual Studio Code — кэш",
            "Кэш интерфейса и загруженных расширений VS Code.", true, false,
            ElectronCaches(Path.Combine(Roaming, "Code"))
                .Append(new CleanTarget { Path = Path.Combine(Roaming, @"Code\CachedExtensionVSIXs") })
                .Append(new CleanTarget { Path = Path.Combine(Roaming, @"Code\CachedData") })
                .ToArray()));
        items.Add(Folder("teams", GroupApps, "\uE716", "Microsoft Teams — кэш",
            "Кэш классической версии Teams.", true, false,
            ElectronCaches(Path.Combine(Roaming, @"Microsoft\Teams"))));

        // ---------------- Developers (off by default)
        items.Add(Folder("npm", GroupDev, "\uEC7A", "npm — кэш пакетов", "Скачанные пакеты Node.js. Будут загружены заново при установке.", false, false,
            new CleanTarget { Path = Path.Combine(Local, "npm-cache") }));
        items.Add(Folder("yarn", GroupDev, "\uEC7A", "Yarn — кэш пакетов", "Кэш менеджера пакетов Yarn.", false, false,
            new CleanTarget { Path = Path.Combine(Local, @"Yarn\Cache") }));
        items.Add(Folder("pip", GroupDev, "\uEC7A", "pip — кэш пакетов", "Скачанные пакеты Python.", false, false,
            new CleanTarget { Path = Path.Combine(Local, @"pip\Cache") }));
        items.Add(Folder("nuget", GroupDev, "\uEC7A", "NuGet — HTTP-кэш", "Временный кэш загрузок NuGet (установленные пакеты не трогаем).", false, false,
            new CleanTarget { Path = Path.Combine(Local, @"NuGet\v3-cache") },
            new CleanTarget { Path = Path.Combine(Local, @"NuGet\plugins-cache") }));
        items.Add(Folder("gradle", GroupDev, "\uEC7A", "Gradle — кэш", "Кэш сборок и зависимостей Gradle.", false, false,
            new CleanTarget { Path = Path.Combine(Profile, @".gradle\caches") }));
        items.Add(Folder("gobuild", GroupDev, "\uEC7A", "Go — кэш сборки", "Промежуточные файлы компилятора Go.", false, false,
            new CleanTarget { Path = Path.Combine(Local, "go-build") }));
        items.Add(Folder("cargo", GroupDev, "\uEC7A", "Cargo — кэш реестра", "Скачанные архивы крейтов Rust.", false, false,
            new CleanTarget { Path = Path.Combine(Profile, @".cargo\registry\cache") }));

        return items.Where(i => i.IsRecycleBin || i.CustomMeasure != null || i.Targets.Any(Exists)).ToList();
    }

    /// <summary>%TEMP%, accepted only when it really is a dedicated temp folder.</summary>
    private static string UserTempPath()
    {
        string p = Path.GetTempPath().TrimEnd('\\');
        string name = Path.GetFileName(p);
        return name.Equals("Temp", StringComparison.OrdinalIgnoreCase) || name.Equals("Tmp", StringComparison.OrdinalIgnoreCase)
            ? p
            : Path.Combine(Local, "Temp");
    }

    private static CleanupItem Folder(string id, string group, string glyph, string title, string description, bool check,
        bool admin, params CleanTarget[] targets) =>
        new()
        {
            Id = id, Group = group, Glyph = glyph, Title = title, Description = description,
            RequiresAdmin = admin, IsChecked = check && (!admin || ShellHelper.IsAdmin),
            Targets = targets.Where(Exists).ToList()
        };

    private static CleanTarget[] ElectronCaches(string root) =>
        new[] { "Cache", "Code Cache", "GPUCache", "DawnCache", "DawnGraphiteCache", "DawnWebGPUCache", "Service Worker\\CacheStorage" }
            .Select(c => new CleanTarget { Path = Path.Combine(root, c) })
            .ToArray();

    private static void AddChromium(List<CleanupItem> items, string id, string name, params string[] userDataDirs)
    {
        var targets = new List<CleanTarget>();
        foreach (var userData in userDataDirs.Where(Directory.Exists))
        {
            foreach (var c in ChromiumRootCaches) targets.Add(new CleanTarget { Path = Path.Combine(userData, c) });
            var profiles = new List<string> { userData };
            try
            {
                profiles.AddRange(Directory.GetDirectories(userData).Where(d =>
                {
                    string n = Path.GetFileName(d);
                    return n == "Default" || n.StartsWith("Profile ", StringComparison.Ordinal) || n == "Guest Profile" || n == "System Profile";
                }));
            }
            catch { }
            foreach (var p in profiles)
                foreach (var c in ChromiumProfileCaches)
                    targets.Add(new CleanTarget { Path = Path.Combine(p, c) });
        }
        // Opera keeps its disk cache under LocalAppData while profile data lives in Roaming.
        if (targets.Count == 0) return;
        items.Add(Folder(id, GroupBrowsers, "\uE774", name + " — кэш",
            "Кэш страниц, скриптов и шейдеров. Пароли, история, закладки и вкладки не затрагиваются.", true, false,
            targets.ToArray()));
    }

    private static void AddFirefox(List<CleanupItem> items)
    {
        string profiles = Path.Combine(Local, @"Mozilla\Firefox\Profiles");
        if (!Directory.Exists(profiles)) return;
        var targets = new List<CleanTarget>();
        try
        {
            foreach (var p in Directory.GetDirectories(profiles))
            {
                targets.Add(new CleanTarget { Path = Path.Combine(p, "cache2") });
                targets.Add(new CleanTarget { Path = Path.Combine(p, "startupCache") });
                targets.Add(new CleanTarget { Path = Path.Combine(p, "thumbnails") });
                targets.Add(new CleanTarget { Path = Path.Combine(p, "jumpListCache") });
            }
        }
        catch { }
        if (targets.Count == 0) return;
        items.Add(Folder("firefox", GroupBrowsers, "\uE774", "Mozilla Firefox — кэш",
            "Кэш страниц и миниатюр. Пароли, история и закладки не затрагиваются.", true, false, targets.ToArray()));
    }

    private static bool Exists(CleanTarget t) => IsSafe(t) && (t.IsFile ? File.Exists(t.Path) : Directory.Exists(t.Path));

    /// <summary>Guards against misconfigured environment variables pointing at a drive root or a user/system folder.</summary>
    private static bool IsSafe(CleanTarget t)
    {
        try
        {
            if (string.IsNullOrWhiteSpace(t.Path) || !Path.IsPathFullyQualified(t.Path)) return false;
            string full = Path.GetFullPath(t.Path).TrimEnd('\\');
            if (full.Length <= 3) return false;
            if (t.IsFile) return true;
            string[] forbidden =
            {
                WinDir, Environment.SystemDirectory, Profile, Local, Roaming, ProgramData,
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles),
                Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86),
                Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
                Environment.GetFolderPath(Environment.SpecialFolder.Desktop),
                ShellHelper.DownloadsFolder,
            };
            if (forbidden.Any(f => !string.IsNullOrEmpty(f) && string.Equals(f.TrimEnd('\\'), full, StringComparison.OrdinalIgnoreCase)))
                return false;
            return true;
        }
        catch
        {
            return false;
        }
    }

    // ------------------------------------------------------------------ measure & clean

    public static (long size, long count) Measure(CleanupItem item, CancellationToken ct)
    {
        if (item.IsRecycleBin) return ShellHelper.QueryRecycleBin();
        if (item.CustomMeasure != null) return item.CustomMeasure(ct);
        long size = 0, count = 0;
        foreach (var t in item.Targets)
        {
            Walk(t, ct, (path, e) =>
            {
                size += e.DiskSize;
                count++;
                return true;
            });
        }
        return (size, count);
    }

    public static CleanResult Clean(CleanupItem item, CancellationToken ct)
    {
        if (item.CustomClean != null) return item.CustomClean(ct);
        var r = new CleanResult();
        if (item.IsRecycleBin)
        {
            var (size, _) = ShellHelper.QueryRecycleBin();
            if (ShellHelper.EmptyRecycleBin()) r.FreedBytes = size;
            return r;
        }

        foreach (var t in item.Targets)
        {
            Walk(t, ct, (path, e) =>
            {
                if (TryDelete(path, e)) { r.FreedBytes += e.DiskSize; r.DeletedFiles++; }
                else r.FailedFiles++;
                return true;
            });
            if (!t.IsFile && t.Recursive && t.RemoveEmptyDirs) RemoveEmptyDirs(t.Path, isRoot: true);
        }
        return r;
    }

    private static bool TryDelete(string path, FindEntry e)
    {
        try
        {
            if ((e.Attributes & Native.FILE_ATTRIBUTE_READONLY) != 0) File.SetAttributes(path, FileAttributes.Normal);
            File.Delete(path);
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>Visits every file of a target that matches its mask and age filter. Links are never followed.</summary>
    private static void Walk(CleanTarget t, CancellationToken ct, Func<string, FindEntry, bool> onFile)
    {
        if (t.IsFile)
        {
            try
            {
                var fi = new FileInfo(t.Path);
                if (fi.Exists)
                    onFile(t.Path, new FindEntry(fi.Name, (uint)fi.Attributes, fi.Length, fi.LastWriteTime.ToFileTime(), 0));
            }
            catch { }
            return;
        }

        long cutoff = t.MinAge > TimeSpan.Zero ? DateTime.Now.Subtract(t.MinAge).ToFileTime() : long.MaxValue;
        var settings = AppSettings.Current;
        bool checkExclusions = settings.Exclusions.Count > 0;
        if (checkExclusions && settings.IsExcluded(t.Path)) return;
        var stack = new Stack<string>();
        stack.Push(t.Path);
        var buffer = new List<FindEntry>(128);
        while (stack.Count > 0)
        {
            if (ct.IsCancellationRequested) return;
            string dir = stack.Pop();
            buffer.Clear();
            FastDir.List(dir, buffer);
            foreach (var e in buffer)
            {
                if (e.IsDirectory)
                {
                    if (t.Recursive && (e.Attributes & Native.FILE_ATTRIBUTE_REPARSE_POINT) == 0)
                    {
                        string sub = FastDir.Combine(dir, e.Name);
                        if (!checkExclusions || !settings.IsExcluded(sub)) stack.Push(sub);
                    }
                    continue;
                }
                if (t.Pattern != "*" && !FileSystemName.MatchesSimpleExpression(t.Pattern, e.Name)) continue;
                if (cutoff != long.MaxValue && e.LastWrite > cutoff) continue;
                if (!onFile(FastDir.Combine(dir, e.Name), e)) return;
            }
        }
    }

    private static void RemoveEmptyDirs(string dir, bool isRoot)
    {
        try
        {
            var buffer = new List<FindEntry>();
            FastDir.List(dir, buffer);
            foreach (var e in buffer)
            {
                if (e.IsDirectory && (e.Attributes & Native.FILE_ATTRIBUTE_REPARSE_POINT) == 0)
                    RemoveEmptyDirs(FastDir.Combine(dir, e.Name), false);
            }
            if (!isRoot)
            {
                buffer.Clear();
                FastDir.List(dir, buffer);
                if (buffer.Count == 0) Directory.Delete(dir, false);
            }
        }
        catch { }
    }

    /// <summary>Size of the Windows.old folder left after a feature update (informational only).</summary>
    public static long WindowsOldSize()
    {
        string path = Path.Combine(Path.GetPathRoot(WinDir) ?? "C:\\", "Windows.old");
        return Directory.Exists(path) ? FastDir.GetSize(path) : 0;
    }
}
