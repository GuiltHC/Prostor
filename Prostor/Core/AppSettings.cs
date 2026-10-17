using System.Text.Json;
using System.Text.Json.Serialization;

namespace Prostor.Core;

public sealed class WindowBounds
{
    public double Left { get; set; }
    public double Top { get; set; }
    public double Width { get; set; }
    public double Height { get; set; }
    public bool Maximized { get; set; }
}

public sealed class AppSettings
{
    public static AppSettings Current { get; private set; } = new();

    /// <summary>"system", "dark" or "light".</summary>
    public string Theme { get; set; } = "system";
    public bool UseMica { get; set; } = true;
    public bool UseSystemAccent { get; set; }

    public WindowBounds Window { get; set; }
    public string LastPage { get; set; }
    public string ProgramsSort { get; set; }

    // programs
    public bool RestorePointBeforeUninstall { get; set; } = true;
    public bool SearchLeftovers { get; set; } = true;
    public int UnusedMonths { get; set; } = 6;

    // cleanup
    /// <summary>Folders that are never cleaned, deleted or offered as leftovers.</summary>
    public List<string> Exclusions { get; set; } = new();
    /// <summary>Checked state of cleanup items chosen by the user (item id → checked).</summary>
    public Dictionary<string, bool> CleanupChecks { get; set; } = new();
    /// <summary>"off", "weekly" or "monthly".</summary>
    public string AutoClean { get; set; } = "off";
    public List<string> AutoCleanItems { get; set; } = new();
    public DateTime? LastAutoClean { get; set; }
    public long LastAutoCleanFreed { get; set; }

    // startup
    /// <summary>Services switched from automatic to manual start by «Простор» (so they stay in the list).</summary>
    public List<string> ManualServices { get; set; } = new();

    // updates
    public bool CheckUpdates { get; set; } = true;
    public string UpdateRepo { get; set; } = "GuiltHC/Prostor";
    public string SkippedVersion { get; set; }

    [JsonIgnore]
    public static string DataFolder => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Prostor");

    private static string FilePath => Path.Combine(DataFolder, "settings.json");

    private static readonly JsonSerializerOptions Json = new() { WriteIndented = true };

    public static AppSettings Load()
    {
        try
        {
            if (File.Exists(FilePath))
                Current = JsonSerializer.Deserialize<AppSettings>(File.ReadAllText(FilePath)) ?? new AppSettings();
        }
        catch
        {
            Current = new AppSettings();
        }
        Current.Exclusions ??= new();
        Current.CleanupChecks ??= new(StringComparer.OrdinalIgnoreCase);
        Current.AutoCleanItems ??= new();
        Current.ManualServices ??= new();
        if (Current.UnusedMonths <= 0) Current.UnusedMonths = 6;
        return Current;
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(DataFolder);
            string tmp = FilePath + ".tmp";
            File.WriteAllText(tmp, JsonSerializer.Serialize(this, Json));
            File.Move(tmp, FilePath, true);
        }
        catch { }
    }

    /// <summary>True when the path is one of the user's exclusions or lies inside one.</summary>
    public bool IsExcluded(string path)
    {
        if (string.IsNullOrEmpty(path) || Exclusions.Count == 0) return false;
        string p = path.TrimEnd('\\');
        foreach (var e in Exclusions)
        {
            if (string.IsNullOrWhiteSpace(e)) continue;
            string x = e.TrimEnd('\\');
            if (p.Equals(x, StringComparison.OrdinalIgnoreCase) || p.StartsWith(x + "\\", StringComparison.OrdinalIgnoreCase))
                return true;
        }
        return false;
    }
}
