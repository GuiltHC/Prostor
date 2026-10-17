using System.Diagnostics;
using System.Net.Http;
using System.Reflection;
using System.Text.Json;
using System.Windows;

namespace Prostor.Services;

public sealed record ReleaseInfo(string Tag, Version Version, string PageUrl, string AssetUrl, long AssetSize, string Notes);

/// <summary>Checks GitHub releases for a newer version and replaces the running exe (Windows allows renaming it).</summary>
public static class Updater
{
    public static Version CurrentVersion => Assembly.GetExecutingAssembly().GetName().Version ?? new Version(1, 0);

    public static string CurrentVersionText
    {
        get
        {
            var v = CurrentVersion;
            return v.Build > 0 ? $"{v.Major}.{v.Minor}.{v.Build}" : $"{v.Major}.{v.Minor}";
        }
    }

    /// <summary>The framework-dependent build loads System.Private.CoreLib from the shared .NET folder.</summary>
#pragma warning disable IL3000 // an empty Location is exactly what tells the self-contained single file apart
    public static bool IsLiteBuild => !string.IsNullOrEmpty(typeof(object).Assembly.Location);
#pragma warning restore IL3000

    private static string AssetName => IsLiteBuild ? "Prostor-lite.exe" : "Prostor.exe";

    public static async Task<ReleaseInfo> CheckAsync(string repo, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(repo) || !repo.Contains('/')) return null;
        using var http = NewClient();
        using var response = await http.GetAsync($"https://api.github.com/repos/{repo.Trim()}/releases/latest", ct);
        if (response.StatusCode == System.Net.HttpStatusCode.NotFound) return null; // no releases published yet
        response.EnsureSuccessStatusCode();
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync(ct));
        var root = doc.RootElement;
        string tag = root.GetProperty("tag_name").GetString() ?? "";
        if (!Version.TryParse(tag.TrimStart('v', 'V'), out var version)) return null;
        string assetUrl = null;
        long assetSize = 0;
        if (root.TryGetProperty("assets", out var assets))
        {
            foreach (var a in assets.EnumerateArray())
            {
                if (!string.Equals(a.GetProperty("name").GetString(), AssetName, StringComparison.OrdinalIgnoreCase)) continue;
                assetUrl = a.GetProperty("browser_download_url").GetString();
                assetSize = a.TryGetProperty("size", out var s) ? s.GetInt64() : 0;
            }
        }
        var info = new ReleaseInfo(tag, version, root.TryGetProperty("html_url", out var u) ? u.GetString() : null, assetUrl, assetSize,
            root.TryGetProperty("body", out var b) ? b.GetString() : null);
        return Normalize(version) > Normalize(CurrentVersion) ? info : null;
    }

    private static Version Normalize(Version v) => new(v.Major, v.Minor, Math.Max(0, v.Build), Math.Max(0, v.Revision));

    public static async Task InstallAsync(ReleaseInfo release, IProgress<double> progress, CancellationToken ct)
    {
        if (release.AssetUrl == null) throw new InvalidOperationException("В выпуске нет файла для этой версии программы.");
        string exe = Environment.ProcessPath!;
        string tmp = exe + ".download";
        using (var http = NewClient())
        using (var resp = await http.GetAsync(release.AssetUrl, HttpCompletionOption.ResponseHeadersRead, ct))
        {
            resp.EnsureSuccessStatusCode();
            long total = resp.Content.Headers.ContentLength ?? release.AssetSize;
            await using var src = await resp.Content.ReadAsStreamAsync(ct);
            await using var dst = File.Create(tmp);
            var buffer = new byte[1 << 16];
            long done = 0;
            int n;
            while ((n = await src.ReadAsync(buffer, ct)) > 0)
            {
                await dst.WriteAsync(buffer.AsMemory(0, n), ct);
                done += n;
                if (total > 0) progress?.Report((double)done / total);
            }
        }

        string old = OldPath(exe);
        if (File.Exists(old)) File.Delete(old);
        File.Move(exe, old);
        try
        {
            File.Move(tmp, exe);
        }
        catch
        {
            File.Move(old, exe); // put the working copy back
            throw;
        }
        Process.Start(new ProcessStartInfo(exe, "--new-window") { UseShellExecute = true });
        Application.Current.Shutdown();
    }

    /// <summary>Removes the previous exe left after an update.</summary>
    public static void CleanupAfterUpdate()
    {
        try
        {
            string exe = Environment.ProcessPath;
            if (exe == null) return;
            string old = OldPath(exe);
            if (File.Exists(old)) File.Delete(old);
            if (File.Exists(exe + ".download")) File.Delete(exe + ".download");
        }
        catch { }
    }

    private static string OldPath(string exe) =>
        Path.Combine(Path.GetDirectoryName(exe)!, Path.GetFileNameWithoutExtension(exe) + ".old.exe");

    private static HttpClient NewClient()
    {
        var http = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
        http.DefaultRequestHeaders.UserAgent.ParseAdd($"Prostor/{CurrentVersionText}");
        http.DefaultRequestHeaders.Accept.ParseAdd("application/vnd.github+json");
        return http;
    }
}
