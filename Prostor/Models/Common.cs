using System.Windows.Media;
using Prostor.Core;

namespace Prostor.Models;

public sealed class NavItem : ObservableObject
{
    public string Key { get; init; }
    public string Title { get; init; }
    public string Glyph { get; init; }
    public string Hotkey { get; init; }
    public string ToolTipText => Hotkey == null ? Title : $"{Title} ({Hotkey})";

    private string _badge;
    public string Badge { get => _badge; set => Set(ref _badge, value); }
}

public sealed class SelectableItem : ObservableObject
{
    public string Title { get; init; }
    public string Subtitle { get; init; }
    public string Detail { get; init; }
    public long Size { get; init; }
    public object Tag { get; init; }

    private bool _isChecked = true;
    public bool IsChecked { get => _isChecked; set => Set(ref _isChecked, value); }
}

public sealed class DriveItem : ObservableObject
{
    public string Name { get; init; }          // "C:"
    public string RootPath { get; init; }      // "C:\\"
    public string Label { get; init; }
    public long Total { get; init; }
    public long Free { get; init; }
    public bool IsRemovable { get; init; }
    public string Format { get; init; }

    public long Used => Math.Max(0, Total - Free);
    public double UsedFraction => Total > 0 ? (double)Used / Total : 0;
    public string Title => $"{Label} ({Name})";
    public string FreeText => $"Свободно {Fmt.Size(Free)} из {Fmt.Size(Total)}";
    public string UsedText => $"Занято {Fmt.Size(Used)}";
    public string PercentText => Math.Round(UsedFraction * 100).ToString("0") + "%";
    public string Glyph => IsRemovable ? "" : "";

    /// <summary>"ok" / "warn" / "danger" — drives the color of the usage indicator.</summary>
    public string Level => UsedFraction >= 0.95 ? "danger" : UsedFraction >= 0.85 ? "warn" : "ok";

    // ---- free space history and forecast
    private List<double> _history;
    public List<double> History { get => _history; set => Set(ref _history, value); }

    private string _forecastText;
    public string ForecastText { get => _forecastText; set => Set(ref _forecastText, value); }

    private string _forecastLevel = "ok";
    public string ForecastLevel { get => _forecastLevel; set => Set(ref _forecastLevel, value); }

    // ---- physical disk health
    private string _healthText;
    public string HealthText { get => _healthText; set => Set(ref _healthText, value); }

    private string _healthLevel;
    /// <summary>"ok", "warn", "danger" or null when unknown.</summary>
    public string HealthLevel { get => _healthLevel; set => Set(ref _healthLevel, value); }

    private string _diskModel;
    public string DiskModel { get => _diskModel; set => Set(ref _diskModel, value); }
}

public sealed class Recommendation
{
    public string Glyph { get; init; }
    /// <summary>"accent", "success", "warning", "danger", "info".</summary>
    public string Tone { get; init; } = "accent";
    public string Title { get; init; }
    public string Detail { get; init; }
    public long Size { get; init; }
    public string SizeText => Size > 0 ? Fmt.Size(Size) : null;
    public string ActionText { get; init; }
    public Action Action { get; init; }
}

public sealed class BarItem
{
    public InstalledApp App { get; init; }
    public string Title { get; init; }
    public string Value { get; init; }
    public double Fraction { get; init; }
    public Brush Brush { get; init; }
}
