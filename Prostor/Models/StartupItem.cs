using System.Windows.Media;
using Microsoft.Win32;
using Prostor.Core;

namespace Prostor.Models;

public enum StartupKind { Run, Folder, Task, Service }

public sealed class StartupItem : ObservableObject
{
    public StartupKind Kind { get; init; }
    public string Name { get; init; }
    public string Command { get; init; }
    public string Arguments { get; init; }
    public string FilePath { get; init; }
    public string Publisher { get; init; }
    public string Description { get; init; }
    public string SourceText { get; init; }

    /// <summary>Hive that holds the StartupApproved value.</summary>
    public RegistryHive ApprovedHive { get; init; }
    /// <summary>"Run", "Run32" or "StartupFolder".</summary>
    public string ApprovedKey { get; init; }
    public string ApprovedValueName { get; init; }

    // scheduled task
    public string TaskPath { get; init; }
    public bool TaskOwnedByUser { get; init; }

    // service
    public string ServiceName { get; init; }
    public bool ServiceDelayed { get; init; }

    /// <summary>Stable identity used by the journal: "hive|approvedKey|valueName".</summary>
    public string Key => Kind switch
    {
        StartupKind.Task => "task|" + TaskPath,
        StartupKind.Service => "service|" + ServiceName,
        _ => $"{ApprovedHive}|{ApprovedKey}|{ApprovedValueName}"
    };

    public string GroupName => Kind switch
    {
        StartupKind.Task => "Задачи планировщика",
        StartupKind.Service => "Службы",
        _ => "Программы"
    };

    public bool CanModify => Kind switch
    {
        StartupKind.Service => ShellHelper.IsAdmin,
        StartupKind.Task => TaskOwnedByUser || ShellHelper.IsAdmin,
        _ => ApprovedHive == RegistryHive.CurrentUser || ShellHelper.IsAdmin
    };

    public string LockHint => CanModify ? null : Kind switch
    {
        StartupKind.Service => "Изменять службы можно только в режиме администратора",
        StartupKind.Task => "Задача создана для всех пользователей — нужны права администратора",
        _ => "Запись для всех пользователей — нужны права администратора"
    };

    /// <summary>Programs from Run / Startup folder can be moved to a delayed start.</summary>
    public bool CanDelay => Kind is StartupKind.Run or StartupKind.Folder && !string.IsNullOrEmpty(FilePath);

    private string _delayTaskPath;
    /// <summary>Set when the program starts from a «Простор» task a minute after logon instead.</summary>
    public string DelayTaskPath
    {
        get => _delayTaskPath;
        set
        {
            if (Set(ref _delayTaskPath, value))
            {
                OnPropertyChanged(nameof(IsDelayed));
                OnPropertyChanged(nameof(StateText));
                OnPropertyChanged(nameof(DelayMenuText));
            }
        }
    }

    public bool IsDelayed => _delayTaskPath != null;
    public string DelayMenuText => IsDelayed ? "Запускать сразу при входе" : "Запускать через минуту после входа";

    public string Title => string.IsNullOrWhiteSpace(Description) ? Name : Description;
    public string Subtitle
    {
        get
        {
            var parts = new List<string>();
            if (!string.IsNullOrWhiteSpace(Description) && !string.Equals(Description, Name, StringComparison.OrdinalIgnoreCase))
                parts.Add(Name);
            if (!string.IsNullOrWhiteSpace(Publisher)) parts.Add(Publisher);
            return string.Join("  ·  ", parts);
        }
    }

    private bool _isEnabled;
    public bool IsEnabled
    {
        get => _isEnabled;
        set
        {
            if (Set(ref _isEnabled, value)) OnPropertyChanged(nameof(StateText));
        }
    }

    public string StateText => !IsEnabled ? "Выкл." : IsDelayed ? "Через 1 мин" : Kind == StartupKind.Service && ServiceDelayed ? "Отложенно" : "Вкл.";

    private ImageSource _icon;
    public ImageSource Icon { get => _icon; set => Set(ref _icon, value); }

    public string Initial
    {
        get
        {
            string t = (Title ?? "").Trim();
            return t.Length == 0 ? "?" : char.ToUpperInvariant(t[0]).ToString();
        }
    }

    public string KindGlyph => Kind switch
    {
        StartupKind.Task => "",
        StartupKind.Service => "",
        _ => null
    };
}

public sealed class BootRecord
{
    public DateTime Time { get; init; }
    /// <summary>Until the desktop is usable, seconds.</summary>
    public double MainPathSeconds { get; init; }
    /// <summary>Including programs that start after logon, seconds.</summary>
    public double TotalSeconds { get; init; }
    public int StartupApps { get; init; }
}
