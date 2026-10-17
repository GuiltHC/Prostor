using System.Text.Json;
using Microsoft.Win32;
using Prostor.Models;

namespace Prostor.Services;

/// <summary>Games of Epic, GOG, EA, Ubisoft, Battle.net and Xbox: correct folders, sizes and the way to uninstall them.</summary>
public static class GameLaunchers
{
    public const string Epic = "Epic Games";
    public const string Gog = "GOG";
    public const string Ea = "EA";
    public const string Ubisoft = "Ubisoft Connect";
    public const string BattleNet = "Battle.net";
    public const string Xbox = "Xbox";

    public static string EpicManifests => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.CommonApplicationData), @"Epic\EpicGamesLauncher\Data\Manifests");

    public static void Attach(List<InstalledApp> apps)
    {
        try { AttachEpic(apps); } catch { }
        try { TagGog(apps); } catch { }
        try { TagUbisoft(apps); } catch { }
        try { TagEa(apps); } catch { }
        try { TagBattleNet(apps); } catch { }
    }

    // ------------------------------------------------------------------ Epic

    public sealed record EpicGame(string AppName, string DisplayName, string InstallLocation, long InstallSize, string Manifest,
        string CatalogNamespace, string CatalogItemId);

    public static List<EpicGame> LoadEpic()
    {
        var list = new List<EpicGame>();
        if (!Directory.Exists(EpicManifests)) return list;
        foreach (var file in Directory.GetFiles(EpicManifests, "*.item"))
        {
            try
            {
                var g = ParseEpicManifest(File.ReadAllText(file), file);
                if (g != null) list.Add(g);
            }
            catch { }
        }
        return list;
    }

    public static EpicGame ParseEpicManifest(string json, string file)
    {
        using var doc = JsonDocument.Parse(json);
        var r = doc.RootElement;
        string Str(string n) => r.TryGetProperty(n, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() : null;
        bool Bool(string n) => r.TryGetProperty(n, out var v) && v.ValueKind == JsonValueKind.True;
        string app = Str("AppName"), main = Str("MainGameAppName");
        if (string.IsNullOrEmpty(app) || Bool("bIsIncompleteInstall")) return null;
        if (!string.IsNullOrEmpty(main) && !string.Equals(app, main, StringComparison.OrdinalIgnoreCase)) return null; // DLC
        long size = r.TryGetProperty("InstallSize", out var s) && s.TryGetInt64(out long sz) ? sz : 0;
        return new EpicGame(app, Str("DisplayName") ?? app, Str("InstallLocation"), size, file, Str("CatalogNamespace"), Str("CatalogItemId"));
    }

    private static void AttachEpic(List<InstalledApp> apps)
    {
        foreach (var g in LoadEpic())
        {
            if (string.IsNullOrEmpty(g.InstallLocation) || !Directory.Exists(g.InstallLocation)) continue;
            string loc = g.InstallLocation.TrimEnd('\\', '/');
            // some games register an uninstall entry too: enrich it instead of duplicating
            var existing = apps.FirstOrDefault(a => a.InstallLocation != null &&
                                                    a.InstallLocation.TrimEnd('\\').Equals(loc, StringComparison.OrdinalIgnoreCase));
            if (existing != null)
            {
                existing.GameSource = Epic;
                continue;
            }
            var app = new InstalledApp
            {
                Id = "epic|" + g.AppName,
                Name = g.DisplayName,
                Publisher = Epic,
                InstallLocation = loc,
                UninstallString = "com.epicgames.launcher://store/library",
                InstallDate = File.GetLastWriteTime(g.Manifest),
            };
            app.GameSource = Epic;
            app.LauncherName = "Epic Games Launcher";
            app.LauncherManifest = g.Manifest;
            if (g.InstallSize > 0) app.SizeBytes = g.InstallSize;
            apps.Add(app);
        }
    }

    // ------------------------------------------------------------------ GOG / EA / Ubisoft / Battle.net (they register normal uninstall entries)

    private static void TagGog(List<InstalledApp> apps)
    {
        using var b = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry32);
        using var root = b.OpenSubKey(@"SOFTWARE\GOG.com\Games");
        if (root == null) return;
        foreach (var id in root.GetSubKeyNames())
        {
            using var k = root.OpenSubKey(id);
            string path = (k?.GetValue("path") as string)?.TrimEnd('\\');
            if (string.IsNullOrEmpty(path)) continue;
            foreach (var a in apps.Where(a => SameFolder(a.InstallLocation, path))) a.GameSource = Gog;
        }
    }

    private static void TagUbisoft(List<InstalledApp> apps)
    {
        foreach (var a in apps.Where(a => a.UninstallString?.Contains("uplay://uninstall", StringComparison.OrdinalIgnoreCase) == true))
        {
            a.GameSource = Ubisoft;
            a.LauncherName = Ubisoft;
        }
        using var b = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry32);
        using var root = b.OpenSubKey(@"SOFTWARE\Ubisoft\Launcher\Installs");
        if (root == null) return;
        foreach (var id in root.GetSubKeyNames())
        {
            using var k = root.OpenSubKey(id);
            string path = (k?.GetValue("InstallDir") as string)?.Replace('/', '\\').TrimEnd('\\');
            if (string.IsNullOrEmpty(path)) continue;
            foreach (var a in apps.Where(a => SameFolder(a.InstallLocation, path))) a.GameSource = Ubisoft;
        }
    }

    private static void TagEa(List<InstalledApp> apps)
    {
        foreach (var a in apps.Where(a => a.UninstallString?.Contains(@"EAInstaller\", StringComparison.OrdinalIgnoreCase) == true ||
                                          a.UninstallString?.Contains(@"__Installer\", StringComparison.OrdinalIgnoreCase) == true))
            a.GameSource ??= Ea;
    }

    private static void TagBattleNet(List<InstalledApp> apps)
    {
        foreach (var a in apps.Where(a => a.UninstallString?.Contains("Blizzard Uninstaller", StringComparison.OrdinalIgnoreCase) == true &&
                                          !string.Equals(a.Name, "Battle.net", StringComparison.OrdinalIgnoreCase)))
            a.GameSource = BattleNet;
    }

    /// <summary>Xbox / Game Pass titles are Store packages installed into an XboxGames folder.</summary>
    public static void TagXbox(List<InstalledApp> apps)
    {
        foreach (var a in apps.Where(a => a.IsStore && a.InstallLocation != null &&
                                          a.InstallLocation.Contains(@"\XboxGames\", StringComparison.OrdinalIgnoreCase)))
            a.GameSource = Xbox;
    }

    private static bool SameFolder(string a, string b) =>
        !string.IsNullOrEmpty(a) && !string.IsNullOrEmpty(b) &&
        string.Equals(a.TrimEnd('\\'), b.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase);
}
