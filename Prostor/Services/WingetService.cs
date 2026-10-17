using System.Text;
using System.Text.RegularExpressions;
using Prostor.Core;
using Prostor.Models;

namespace Prostor.Services;

public sealed record WingetUpgrade(string Name, string Id, string Version, string Available, string Source);

/// <summary>Updates and export through the Windows Package Manager (winget).</summary>
public static class WingetService
{
    private static string _exe;

    public static string Exe
    {
        get
        {
            if (_exe != null) return _exe.Length == 0 ? null : _exe;
            string alias = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), @"Microsoft\WindowsApps\winget.exe");
            _exe = File.Exists(alias) ? alias : "";
            if (_exe.Length == 0)
            {
                try
                {
                    var r = Cmd.Run("where.exe", "winget", Encoding.UTF8, 10_000);
                    string first = r.Output.Split('\n', StringSplitOptions.RemoveEmptyEntries).FirstOrDefault()?.Trim();
                    if (r.Ok && File.Exists(first)) _exe = first;
                }
                catch { }
            }
            return _exe.Length == 0 ? null : _exe;
        }
    }

    public static bool IsAvailable => Exe != null;

    public static async Task<List<WingetUpgrade>> GetUpgradesAsync(CancellationToken ct = default)
    {
        if (!IsAvailable) return new();
        var r = await Cmd.RunAsync(Exe, "upgrade --include-unknown --accept-source-agreements --disable-interactivity", Encoding.UTF8, 180_000, null, ct);
        return ParseTable(r.Output);
    }

    /// <summary>Parses winget's fixed-width table (column headers are localized, so positions come from the header line).</summary>
    public static List<WingetUpgrade> ParseTable(string output)
    {
        var list = new List<WingetUpgrade>();
        var lines = output.Replace("\r\n", "\n").Split('\n')
            .Select(l => l.Contains('\r') ? l[(l.LastIndexOf('\r') + 1)..] : l)
            .ToList();
        int sep = lines.FindIndex(l => l.Length > 20 && l.Trim().Trim('-').Length == 0 && l.Trim().Length > 20);
        if (sep <= 0) return list;
        string header = lines[sep - 1];
        var starts = new List<int>();
        for (int i = 0; i < header.Length; i++)
            if (header[i] != ' ' && (i == 0 || header[i - 1] == ' ')) starts.Add(i);
        if (starts.Count < 4) return list;

        for (int i = sep + 1; i < lines.Count; i++)
        {
            string l = lines[i];
            if (string.IsNullOrWhiteSpace(l)) break;
            if (l.Length < starts[^1]) continue; // summary line ("45 upgrades available.")
            string Col(int c)
            {
                int a = starts[c];
                int b = c + 1 < starts.Count ? starts[c + 1] : l.Length;
                if (a >= l.Length) return "";
                return l.Substring(a, Math.Min(b, l.Length) - a).Trim();
            }
            string name = Col(0), id = Col(1), version = Col(2), available = Col(3), source = starts.Count > 4 ? Col(4) : "";
            if (string.IsNullOrEmpty(id) || string.IsNullOrEmpty(available) || id.Contains(' ')) continue;
            list.Add(new WingetUpgrade(name, id, version, available, source));
        }
        return list;
    }

    /// <summary>Matches winget rows to installed programs by name (winget shortens long names with "…").</summary>
    public static void Apply(IEnumerable<InstalledApp> apps, IReadOnlyList<WingetUpgrade> upgrades)
    {
        var list = apps.Where(a => !a.IsGame).ToList();
        foreach (var a in list)
        {
            a.UpdateVersion = null;
            a.WingetId = null;
        }
        foreach (var u in upgrades)
        {
            string un = Norm(u.Name.TrimEnd('…'));
            bool truncated = u.Name.EndsWith('…');
            var match = list.FirstOrDefault(a => Norm(a.Name) == un && VersionMatches(a, u)) ??
                        list.FirstOrDefault(a => Norm(a.Name) == un) ??
                        (truncated && un.Length >= 8 ? list.FirstOrDefault(a => Norm(a.Name).StartsWith(un)) : null);
            if (match == null || match.UpdateVersion != null) continue;
            match.UpdateVersion = u.Available;
            match.WingetId = u.Id;
        }
    }

    private static bool VersionMatches(InstalledApp a, WingetUpgrade u) =>
        string.IsNullOrEmpty(a.Version) || u.Version.StartsWith("<") || a.Version.StartsWith(u.Version.TrimEnd('…'), StringComparison.OrdinalIgnoreCase);

    private static string Norm(string s) => new string((s ?? "").Where(char.IsLetterOrDigit).ToArray()).ToLowerInvariant();

    private static readonly Regex Percent = new(@"(\d{1,3})\s?%", RegexOptions.Compiled);

    /// <summary>Installs the newest version. <paramref name="progress"/> gets 0..1 when winget prints a percentage.</summary>
    public static async Task<CmdResult> UpgradeAsync(string id, IProgress<(double? percent, string line)> progress, CancellationToken ct)
    {
        string args = $"upgrade --id {Cmd.Quote(id)} --exact --silent --accept-package-agreements --accept-source-agreements --disable-interactivity";
        return await Cmd.RunAsync(Exe, args, Encoding.UTF8, 3600_000, line =>
        {
            string l = line.Contains('\r') ? line[(line.LastIndexOf('\r') + 1)..] : line;
            l = l.Trim();
            if (l.Length == 0 || l.Trim('-', '\\', '|', '/').Length == 0) return;
            var m = Percent.Match(l);
            double? p = m.Success && double.TryParse(m.Groups[1].Value, out double v) ? Math.Clamp(v / 100, 0, 1) : null;
            string text = Regex.Replace(l, @"[█▒░▓■]+", "").Trim();
            progress?.Report((p, text));
        }, ct);
    }

    public static Task<CmdResult> ExportAsync(string file) =>
        Cmd.RunAsync(Exe, $"export -o {Cmd.Quote(file)} --accept-source-agreements --disable-interactivity", Encoding.UTF8, 300_000);

    /// <summary>winget exit codes that mean "nothing to do" are not errors.</summary>
    public static bool IsSuccess(CmdResult r) => r.ExitCode is 0 or unchecked((int)0x8A15002B) or unchecked((int)0x8A150011);
}

/// <summary>System restore point before uninstalling.</summary>
public enum RestorePointOutcome { Created, Recent, Failed }

public static class RestorePointService
{
    /// <summary>
    /// <see cref="RestorePointOutcome.Recent"/>: Windows makes at most one point a day and quietly skipped this one
    /// (the call still succeeds); the message then names the existing point the system can return to.
    /// </summary>
    public static (RestorePointOutcome outcome, string message) Create(string description)
    {
        if (!ShellHelper.IsAdmin) return (RestorePointOutcome.Failed, "Нужен режим администратора.");
        var before = Latest();
        string error = Set(description);
        if (error != null) return (RestorePointOutcome.Failed, error);
        var after = Latest();
        if (before != null && (after == null || after.Value.sequence <= before.Value.sequence))
            return (RestorePointOutcome.Recent, $"Точка восстановления уже есть — от {before.Value.time:d MMMM, HH:mm}");
        return (RestorePointOutcome.Created, null);
    }

    /// <summary>The newest restore point, or null when there is none or it cannot be read.</summary>
    private static (uint sequence, DateTime time)? Latest()
    {
        try
        {
            using var searcher = new System.Management.ManagementObjectSearcher(@"root\default", "SELECT SequenceNumber, CreationTime FROM SystemRestore");
            (uint, DateTime)? latest = null;
            foreach (var o in searcher.Get())
            {
                using (o)
                {
                    uint seq = Convert.ToUInt32(o["SequenceNumber"]);
                    if (latest != null && seq <= latest.Value.Item1) continue;
                    latest = (seq, System.Management.ManagementDateTimeConverter.ToDateTime((string)o["CreationTime"]));
                }
            }
            return latest;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>Returns null on success, otherwise the reason.</summary>
    private static string Set(string description)
    {
        try
        {
            var info = new Native.RESTOREPOINTINFOW
            {
                dwEventType = Native.BEGIN_SYSTEM_CHANGE,
                dwRestorePtType = Native.APPLICATION_UNINSTALL,
                llSequenceNumber = 0,
                szDescription = description.Length > 250 ? description[..250] : description
            };
            if (!Native.SRSetRestorePointW(ref info, out var status))
            {
                return status.nStatus switch
                {
                    1058 => "Защита системы выключена — включите её в «Свойствах системы → Защита системы».",
                    112 => "На диске недостаточно места для точки восстановления.",
                    _ => $"Windows не создала точку восстановления (код {status.nStatus})."
                };
            }
            var end = new Native.RESTOREPOINTINFOW
            {
                dwEventType = Native.END_SYSTEM_CHANGE,
                dwRestorePtType = Native.APPLICATION_UNINSTALL,
                llSequenceNumber = status.llSequenceNumber,
                szDescription = ""
            };
            Native.SRSetRestorePointW(ref end, out _);
            return null;
        }
        catch (DllNotFoundException)
        {
            return "Восстановление системы недоступно в этой версии Windows.";
        }
        catch (Exception ex)
        {
            return ex.Message;
        }
    }
}
