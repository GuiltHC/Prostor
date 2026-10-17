using System.Text;
using Microsoft.Win32;
using Prostor.Models;

namespace Prostor.Services;

public sealed class SteamGame
{
    public string AppId { get; init; }
    public string Name { get; init; }
    public string InstallDir { get; init; }
    public string Library { get; init; }
    public string ManifestPath { get; init; }
    public long SizeOnDisk { get; init; }
    public DateTime? LastUpdated { get; init; }

    public string GamePath => Path.Combine(Library, "steamapps", "common", InstallDir ?? "");

    /// <summary>Workshop content and the shader cache that belong to the game.</summary>
    public IEnumerable<string> ExtraPaths => new[]
    {
        Path.Combine(Library, "steamapps", "workshop", "content", AppId),
        Path.Combine(Library, "steamapps", "shadercache", AppId),
    }.Where(Directory.Exists);
}

/// <summary>Reads Steam libraries (libraryfolders.vdf) and app manifests (appmanifest_*.acf).</summary>
public static class SteamLibrary
{
    private static readonly HashSet<string> NotGames = new() { "228980" }; // Steamworks Common Redistributables

    public static string SteamPath
    {
        get
        {
            try
            {
                using var cu = Registry.CurrentUser.OpenSubKey(@"Software\Valve\Steam");
                if (cu?.GetValue("SteamPath") is string p && Directory.Exists(p)) return Path.GetFullPath(p.Replace('/', '\\'));
                using var lm = RegistryKey.OpenBaseKey(RegistryHive.LocalMachine, RegistryView.Registry32).OpenSubKey(@"SOFTWARE\Valve\Steam");
                if (lm?.GetValue("InstallPath") is string p2 && Directory.Exists(p2)) return Path.GetFullPath(p2);
            }
            catch { }
            return null;
        }
    }

    public static string SteamExe
    {
        get
        {
            string p = SteamPath;
            return p == null ? null : Path.Combine(p, "steam.exe");
        }
    }

    public static List<SteamGame> LoadGames()
    {
        var result = new List<SteamGame>();
        string steam = SteamPath;
        if (steam == null) return result;

        var libraries = new List<string> { steam };
        try
        {
            string vdf = Path.Combine(steam, "steamapps", "libraryfolders.vdf");
            if (File.Exists(vdf) && Parse(File.ReadAllText(vdf)).TryGetValue("libraryfolders", out var lf) && lf is Dictionary<string, object> folders)
            {
                foreach (var entry in folders.Values.OfType<Dictionary<string, object>>())
                    if (entry.TryGetValue("path", out var path) && path is string s && Directory.Exists(s))
                        libraries.Add(Path.GetFullPath(s));
            }
        }
        catch { }

        foreach (var lib in libraries.Distinct(StringComparer.OrdinalIgnoreCase))
        {
            string apps = Path.Combine(lib, "steamapps");
            if (!Directory.Exists(apps)) continue;
            string[] manifests;
            try { manifests = Directory.GetFiles(apps, "appmanifest_*.acf"); }
            catch { continue; }
            foreach (var m in manifests)
            {
                try
                {
                    if (!Parse(File.ReadAllText(m)).TryGetValue("AppState", out var st) || st is not Dictionary<string, object> state) continue;
                    string Get(string k) => state.TryGetValue(k, out var v) ? v as string : null;
                    string id = Get("appid");
                    if (string.IsNullOrEmpty(id) || NotGames.Contains(id)) continue;
                    long.TryParse(Get("SizeOnDisk"), out long size);
                    DateTime? updated = long.TryParse(Get("LastUpdated"), out long unix) && unix > 0
                        ? DateTimeOffset.FromUnixTimeSeconds(unix).LocalDateTime
                        : null;
                    result.Add(new SteamGame
                    {
                        AppId = id,
                        Name = Get("name") ?? $"Steam App {id}",
                        InstallDir = Get("installdir"),
                        Library = lib,
                        ManifestPath = m,
                        SizeOnDisk = size,
                        LastUpdated = updated
                    });
                }
                catch { }
            }
        }
        return result;
    }

    /// <summary>
    /// Links "Steam App N" uninstall entries to their game folders (Steam does not register InstallLocation)
    /// and adds installed games that have no uninstall entry at all.
    /// </summary>
    public static void Attach(List<InstalledApp> apps)
    {
        List<SteamGame> games;
        try { games = LoadGames(); }
        catch { return; }
        if (games.Count == 0) return;

        var byId = games.GroupBy(g => g.AppId).ToDictionary(g => g.Key, g => g.First());
        var matched = new HashSet<string>();
        foreach (var app in apps)
        {
            string id = SteamAppIdOf(app);
            if (id == null || !byId.TryGetValue(id, out var game)) continue;
            Link(app, game);
            matched.Add(id);
        }

        string exe = SteamExe;
        if (exe == null || !File.Exists(exe)) return;
        foreach (var game in games.Where(g => !matched.Contains(g.AppId) && Directory.Exists(g.GamePath)))
        {
            var app = new InstalledApp
            {
                Id = "steam|" + game.AppId,
                Name = game.Name,
                Publisher = "Steam",
                InstallDate = game.LastUpdated,
                UninstallString = $"\"{exe}\" steam://uninstall/{game.AppId}",
            };
            Link(app, game);
            apps.Add(app);
        }
    }

    private static void Link(InstalledApp app, SteamGame game)
    {
        app.SteamAppId = game.AppId;
        app.SteamManifest = game.ManifestPath;
        if (Directory.Exists(game.GamePath)) app.InstallLocation = game.GamePath;
        app.ExtraLocations = game.ExtraPaths.ToList();
        if (app.SizeBytes <= 0 && game.SizeOnDisk > 0) app.SizeBytes = game.SizeOnDisk;
    }

    private static string SteamAppIdOf(InstalledApp app)
    {
        string key = app.RegistrySubKey;
        if (key == null) return null;
        int idx = key.LastIndexOf('\\');
        string name = idx >= 0 ? key.Substring(idx + 1) : key;
        return name.StartsWith("Steam App ", StringComparison.OrdinalIgnoreCase) ? name.Substring(10).Trim() : null;
    }

    // ------------------------------------------------------------------ minimal VDF (KeyValues) parser

    public static Dictionary<string, object> Parse(string text)
    {
        int i = 0;
        return ParseObject(text, ref i);
    }

    private static Dictionary<string, object> ParseObject(string s, ref int i)
    {
        var dict = new Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
        while (true)
        {
            string key = NextToken(s, ref i, out bool isClose);
            if (key == null || isClose) return dict;
            string value = NextToken(s, ref i, out _, out bool isOpen);
            if (isOpen) dict[key] = ParseObject(s, ref i);
            else if (value != null) dict[key] = value;
            else return dict;
        }
    }

    private static string NextToken(string s, ref int i, out bool isClose) => NextToken(s, ref i, out isClose, out _);

    private static string NextToken(string s, ref int i, out bool isClose, out bool isOpen)
    {
        isClose = isOpen = false;
        while (i < s.Length)
        {
            char c = s[i];
            if (char.IsWhiteSpace(c)) { i++; continue; }
            if (c == '/' && i + 1 < s.Length && s[i + 1] == '/')
            {
                while (i < s.Length && s[i] != '\n') i++;
                continue;
            }
            if (c == '{') { i++; isOpen = true; return ""; }
            if (c == '}') { i++; isClose = true; return ""; }
            if (c == '"')
            {
                i++;
                var sb = new StringBuilder();
                while (i < s.Length && s[i] != '"')
                {
                    if (s[i] == '\\' && i + 1 < s.Length)
                    {
                        i++;
                        sb.Append(s[i] switch { 'n' => '\n', 't' => '\t', _ => s[i] });
                    }
                    else sb.Append(s[i]);
                    i++;
                }
                i++;
                return sb.ToString();
            }
            int start = i;
            while (i < s.Length && !char.IsWhiteSpace(s[i]) && s[i] != '{' && s[i] != '}' && s[i] != '"') i++;
            return s.Substring(start, i - start);
        }
        return null;
    }
}
