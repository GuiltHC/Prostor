using System.Windows.Media;
using Microsoft.Win32;
using Prostor.Core;

namespace Prostor.Models;

public sealed class InstalledApp : ObservableObject
{
    private static readonly SolidColorBrush[] Palette = MakePalette(
        "#7B6CFF", "#3B82F6", "#10B981", "#F97316", "#EC4899", "#0EA5C6", "#E11D48", "#8B5CF6", "#D4A017", "#14B8A6");

    public string Id { get; init; }
    public string Name { get; init; }
    public string Publisher { get; init; }
    public string Version { get; init; }
    public DateTime? InstallDate { get; init; }
    public string InstallLocation { get; set; }
    public string UninstallString { get; init; }
    public string QuietUninstallString { get; init; }
    public string ModifyPath { get; init; }
    public string DisplayIcon { get; init; }
    public string HelpLink { get; init; }
    public bool IsMsi { get; init; }
    public string ProductCode { get; init; }
    public bool NoRemove { get; init; }

    // Registry origin (desktop programs)
    public RegistryHive Hive { get; init; }
    public RegistryView View { get; init; }
    public string RegistrySubKey { get; init; }

    // Microsoft Store packages
    public bool IsStore { get; init; }
    public string PackageFullName { get; init; }
    public string PackageFamilyName { get; init; }
    public string LogoPath { get; init; }

    // Games (Steam, Epic, GOG, EA, Ubisoft, Battle.net, Xbox)
    public string SteamAppId { get; set; }
    public string SteamManifest { get; set; }
    public List<string> ExtraLocations { get; set; } = new();
    public bool IsSteamGame => SteamAppId != null;
    /// <summary>"Epic Games", "GOG", "EA", "Ubisoft Connect", "Battle.net", "Xbox" (Steam games use <see cref="SteamAppId"/>).</summary>
    public string GameSource { get; set; }
    public bool IsGame => IsSteamGame || GameSource != null;
    public string GameLabel => IsSteamGame ? "Steam" : GameSource;

    /// <summary>Launcher manifest that disappears once the launcher removed the game (Epic).</summary>
    public string LauncherManifest { get; set; }

    private string _launcherName;
    /// <summary>The program that performs the uninstall (Steam, Epic Games Launcher, Ubisoft Connect).</summary>
    public string LauncherName
    {
        get => _launcherName ?? (IsSteamGame || (UninstallString?.Contains("steam://", StringComparison.OrdinalIgnoreCase) ?? false) ? "Steam" : null);
        set => _launcherName = value;
    }

    /// <summary>The uninstall is handed over to a running launcher instead of a separate uninstaller.</summary>
    public bool IsLauncherUninstall => LauncherName != null;

    public bool CanUninstall => !NoRemove && (IsStore || IsMsi || !string.IsNullOrWhiteSpace(UninstallString));
    public bool HasQuiet => !IsLauncherUninstall && (!string.IsNullOrWhiteSpace(QuietUninstallString) || IsMsi);
    public bool CanModify => !IsStore && !string.IsNullOrWhiteSpace(ModifyPath);
    public bool HasLocation => !string.IsNullOrEmpty(InstallLocation);
    public bool HasRegistryKey => !IsStore && RegistrySubKey != null;

    public string RegistryFullPath => !HasRegistryKey ? null :
        (Hive == RegistryHive.LocalMachine ? "HKEY_LOCAL_MACHINE\\" : "HKEY_CURRENT_USER\\") +
        (View == RegistryView.Registry32 && Hive == RegistryHive.LocalMachine && Environment.Is64BitOperatingSystem
            ? RegistrySubKey.Replace(@"SOFTWARE\Microsoft", @"SOFTWARE\WOW6432Node\Microsoft", StringComparison.OrdinalIgnoreCase)
            : RegistrySubKey);

    public string SourceText => IsStore
        ? GameSource == null ? "Microsoft Store" : $"{GameSource} · Microsoft Store"
        : IsSteamGame
            ? $"Steam · ID {SteamAppId}"
        : LauncherManifest != null
            ? GameSource
        : Hive == RegistryHive.CurrentUser
            ? "Только для текущего пользователя"
            : View == RegistryView.Registry32 && Environment.Is64BitOperatingSystem ? "Для всех · 32-бит" : "Для всех · 64-бит";

    public string KindText => IsGame ? $"Игра {GameLabel}" : IsStore ? "Приложение Store" : IsMsi ? "Установщик Windows (MSI)" : "Классическая программа";

    private long _sizeBytes = -1;
    public long SizeBytes
    {
        get => _sizeBytes;
        set
        {
            if (Set(ref _sizeBytes, value))
            {
                OnPropertyChanged(nameof(SizeText));
                OnPropertyChanged(nameof(HasSize));
            }
        }
    }

    private bool _sizeIsMeasured;
    public bool SizeIsMeasured
    {
        get => _sizeIsMeasured;
        set
        {
            if (Set(ref _sizeIsMeasured, value)) OnPropertyChanged(nameof(SizeNote));
        }
    }

    public bool HasSize => _sizeBytes > 0;
    public string SizeText => _sizeBytes > 0 ? Fmt.Size(_sizeBytes) : "—";
    public string SizeNote => _sizeBytes <= 0
        ? "Размер неизвестен"
        : _sizeIsMeasured ? "Подсчитано по папке программы" : "По данным установщика";

    private double _sizeFraction;
    public double SizeFraction { get => _sizeFraction; set => Set(ref _sizeFraction, value); }

    private ImageSource _icon;
    public ImageSource Icon { get => _icon; set => Set(ref _icon, value); }

    private bool _isChecked;
    public bool IsChecked { get => _isChecked; set => Set(ref _isChecked, value); }

    public string Initial
    {
        get
        {
            string t = (Name ?? "").TrimStart('(', '[', ' ', '"');
            return t.Length == 0 ? "?" : char.ToUpperInvariant(t[0]).ToString();
        }
    }
    public Brush FallbackBrush => Palette[(int)((uint)(Name ?? "").GetHashCode() % (uint)Palette.Length)];

    public string Subtitle
    {
        get
        {
            var parts = new List<string>();
            if (IsGame) parts.Add(GameLabel);
            else if (IsStore) parts.Add("Microsoft Store");
            if (!string.IsNullOrWhiteSpace(Publisher) && !string.Equals(Publisher, GameLabel, StringComparison.OrdinalIgnoreCase)) parts.Add(Publisher);
            if (!string.IsNullOrWhiteSpace(Version)) parts.Add(Version);
            return string.Join("  ·  ", parts);
        }
    }

    // ---- usage, hints and updates

    private DateTime? _lastUsed;
    public DateTime? LastUsed
    {
        get => _lastUsed;
        set
        {
            if (!Set(ref _lastUsed, value)) return;
            OnPropertyChanged(nameof(LastUsedText));
            OnPropertyChanged(nameof(LastUsedLong));
            OnPropertyChanged(nameof(LastUsedSortKey));
            OnPropertyChanged(nameof(UsageNote));
        }
    }

    /// <summary>Sort key: programs without launch data go last.</summary>
    public DateTime LastUsedSortKey => _lastUsed ?? DateTime.MaxValue;
    public string LastUsedText => _lastUsed.HasValue ? Fmt.Ago(_lastUsed) : "—";
    public string LastUsedLong => _lastUsed.HasValue ? $"{Fmt.Date(_lastUsed)} ({Fmt.Ago(_lastUsed)})" : "Нет данных";

    private bool _isUnused;
    /// <summary>Not started for longer than the threshold from the settings.</summary>
    public bool IsUnused
    {
        get => _isUnused;
        set
        {
            if (Set(ref _isUnused, value)) OnPropertyChanged(nameof(UsageNote));
        }
    }

    public string UsageNote => _isUnused && _lastUsed.HasValue
        ? $"Не запускалась {Fmt.Ago(_lastUsed).Replace(" назад", "")} — возможно, она больше не нужна."
        : null;

    private Services.AppHint _hint;
    public Services.AppHint Hint
    {
        get => _hint;
        set
        {
            if (!Set(ref _hint, value)) return;
            OnPropertyChanged(nameof(HintLabel));
            OnPropertyChanged(nameof(HintText));
            OnPropertyChanged(nameof(HintTone));
        }
    }

    public string HintLabel => Services.AppAdvisor.Label(_hint);
    public string HintText => Services.AppAdvisor.Explanation(_hint);
    public string HintTone => Services.AppAdvisor.Tone(_hint);

    private string _updateVersion;
    /// <summary>Newer version available through winget.</summary>
    public string UpdateVersion
    {
        get => _updateVersion;
        set
        {
            if (Set(ref _updateVersion, value)) OnPropertyChanged(nameof(HasUpdate));
        }
    }

    /// <summary>winget package id.</summary>
    public string WingetId { get; set; }
    public bool HasUpdate => !string.IsNullOrEmpty(_updateVersion);

    public string DateText => InstallDate.HasValue ? Fmt.Date(InstallDate) : "—";
    public string DateLong => InstallDate.HasValue ? $"{Fmt.Date(InstallDate)} ({Fmt.Ago(InstallDate)})" : "Неизвестно";
    public string AgoText => Fmt.Ago(InstallDate);

    /// <summary>Used by screen readers / UI Automation as the list item name.</summary>
    public override string ToString() => Name;

    private static SolidColorBrush[] MakePalette(params string[] hex) =>
        hex.Select(h =>
        {
            var b = new SolidColorBrush((Color)ColorConverter.ConvertFromString(h));
            b.Freeze();
            return b;
        }).ToArray();
}
