using System.Collections.ObjectModel;
using System.Windows.Input;
using System.Windows.Media;
using Prostor.Core;
using Prostor.Services;

namespace Prostor.ViewModels;

public sealed record HotkeyInfo(string Keys, string Action);

public sealed class SettingsViewModel : ObservableObject
{
    private readonly MainViewModel _main;
    private AppSettings S => AppSettings.Current;

    public ObservableCollection<string> Exclusions { get; } = new();

    public ICommand AddExclusionCommand { get; }
    public ICommand RemoveExclusionCommand { get; }
    public ICommand CheckUpdatesNowCommand { get; }
    public ICommand OpenDataFolderCommand { get; }
    public ICommand RunAutoCleanNowCommand { get; }

    public IReadOnlyList<HotkeyInfo> Hotkeys { get; } = new[]
    {
        new HotkeyInfo("Ctrl + 1…5", "Обзор, Программы, Анализ, Очистка, Автозагрузка"),
        new HotkeyInfo("Ctrl + 6  /  Ctrl + ,", "Журнал  /  Настройки"),
        new HotkeyInfo("Ctrl + F", "Поиск на странице"),
        new HotkeyInfo("F5", "Обновить страницу"),
        new HotkeyInfo("Delete", "Удалить выбранное (программу, файл или папку)"),
        new HotkeyInfo("Backspace  /  Alt + ←", "На уровень выше в анализе диска"),
        new HotkeyInfo("Esc", "Закрыть окно диалога, очистить поиск"),
    };

    public SettingsViewModel(MainViewModel main)
    {
        _main = main;
        AddExclusionCommand = new RelayCommand(() =>
        {
            var dlg = new Microsoft.Win32.OpenFolderDialog { Title = "Папка, которую «Простор» никогда не будет трогать", Multiselect = true };
            if (dlg.ShowDialog() != true) return;
            foreach (var f in dlg.FolderNames)
            {
                if (S.Exclusions.Any(e => e.Equals(f, StringComparison.OrdinalIgnoreCase))) continue;
                S.Exclusions.Add(f);
                Exclusions.Add(f);
            }
            S.Save();
            OnPropertyChanged(nameof(HasExclusions));
        });
        RemoveExclusionCommand = new RelayCommand(p =>
        {
            if (p is not string f) return;
            S.Exclusions.RemoveAll(e => e.Equals(f, StringComparison.OrdinalIgnoreCase));
            Exclusions.Remove(f);
            S.Save();
            OnPropertyChanged(nameof(HasExclusions));
        });
        CheckUpdatesNowCommand = new RelayCommand(async () => await _main.CheckForUpdatesAsync(true));
        OpenDataFolderCommand = new RelayCommand(() =>
        {
            Directory.CreateDirectory(AppSettings.DataFolder);
            ShellHelper.OpenFolder(AppSettings.DataFolder);
        });
        RunAutoCleanNowCommand = new RelayCommand(() =>
        {
            try
            {
                System.Diagnostics.Process.Start(new System.Diagnostics.ProcessStartInfo(Environment.ProcessPath!, "--auto-clean") { UseShellExecute = true });
                _main.ShowToast("Автоочистка запущена — уведомление появится в углу экрана", "info");
            }
            catch (Exception ex)
            {
                _ = _main.ShowError("Не удалось запустить", ex.Message);
            }
        });
    }

    public void Refresh()
    {
        Exclusions.Clear();
        foreach (var e in S.Exclusions) Exclusions.Add(e);
        _explorerMenu = ExplorerIntegration.IsEnabled;
        _autoCleanRegistered = AutoCleanScheduler.IsRegistered;
        foreach (var p in new[]
                 {
                     nameof(ThemeMode), nameof(UseMica), nameof(UseSystemAccent), nameof(AccentPreview), nameof(RestorePoint),
                     nameof(SearchLeftovers), nameof(UnusedMonths), nameof(HasExclusions), nameof(AutoClean), nameof(AutoCleanStatus),
                     nameof(ExplorerMenu), nameof(ExplorerMenuNote), nameof(CheckUpdates), nameof(UpdateRepo), nameof(VersionText)
                 })
            OnPropertyChanged(p);
    }

    public void RaiseTheme() => OnPropertyChanged(nameof(ThemeMode));

    // ------------------------------------------------------------------ appearance

    public string ThemeMode
    {
        get => S.Theme;
        set
        {
            if (value == null || value == S.Theme) return;
            S.Theme = value;
            S.Save();
            ThemeManager.Apply();
            OnPropertyChanged();
        }
    }

    public bool MicaSupported => ThemeManager.MicaSupported;
    public string MicaNote => MicaSupported
        ? "Полупрозрачный фон окна в цветах обоев, как у приложений Windows 11"
        : "Доступно в Windows 11 версии 22H2 и новее";

    public bool UseMica
    {
        get => S.UseMica && MicaSupported;
        set
        {
            if (value == S.UseMica) return;
            S.UseMica = value;
            S.Save();
            ThemeManager.Apply();
            OnPropertyChanged();
        }
    }

    public bool UseSystemAccent
    {
        get => S.UseSystemAccent;
        set
        {
            if (value == S.UseSystemAccent) return;
            S.UseSystemAccent = value;
            S.Save();
            ThemeManager.Apply();
            OnPropertyChanged();
        }
    }

    public Brush AccentPreview
    {
        get
        {
            var c = ThemeManager.SystemAccentPreview;
            if (c == null) return Brushes.Transparent;
            var b = new SolidColorBrush(c.Value);
            b.Freeze();
            return b;
        }
    }

    // ------------------------------------------------------------------ programs

    public bool RestorePoint
    {
        get => S.RestorePointBeforeUninstall;
        set
        {
            S.RestorePointBeforeUninstall = value;
            S.Save();
            OnPropertyChanged();
        }
    }

    public bool IsAdmin => ShellHelper.IsAdmin;

    public bool SearchLeftovers
    {
        get => S.SearchLeftovers;
        set
        {
            S.SearchLeftovers = value;
            S.Save();
            OnPropertyChanged();
        }
    }

    public string UnusedMonths
    {
        get => S.UnusedMonths.ToString();
        set
        {
            if (!int.TryParse(value, out int m) || m == S.UnusedMonths) return;
            S.UnusedMonths = m;
            S.Save();
            OnPropertyChanged();
            _main.Programs.RefreshUsage();
        }
    }

    // ------------------------------------------------------------------ cleanup

    public bool HasExclusions => Exclusions.Count > 0;

    private bool _autoCleanRegistered;

    public string AutoClean
    {
        get => _autoCleanRegistered ? S.AutoClean : "off";
        set
        {
            if (value == null || value == AutoClean) return;
            try
            {
                AutoCleanScheduler.Apply(value);
                S.AutoClean = value;
                S.Save();
                _autoCleanRegistered = value != "off";
                _main.ShowToast(value switch
                {
                    "weekly" => "Автоочистка: по воскресеньям в 12:00 (или при следующем включении компьютера)",
                    "monthly" => "Автоочистка: 1-го числа каждого месяца",
                    _ => "Автоочистка выключена"
                }, "info");
            }
            catch (Exception ex)
            {
                _ = _main.ShowError("Не удалось настроить расписание", ex.Message);
            }
            OnPropertyChanged();
            OnPropertyChanged(nameof(AutoCleanStatus));
        }
    }

    public string AutoCleanStatus
    {
        get
        {
            string what = "Очищаются те же категории, что отмечены на странице «Очистка» (кроме корзины). " +
                          "«Простор» запускается Планировщиком Windows, чистит и сразу закрывается — в фоне ничего не висит.";
            if (S.LastAutoClean is { } last)
                what += $"\nПоследний запуск: {Fmt.Date(last)} в {last:HH:mm}, освобождено {Fmt.Size(S.LastAutoCleanFreed)}.";
            if (_autoCleanRegistered && AutoCleanScheduler.PointsElsewhere)
                what += "\nВнимание: расписание запускает другую копию программы — выберите режим ещё раз, чтобы обновить путь.";
            return what;
        }
    }

    // ------------------------------------------------------------------ integration

    private bool _explorerMenu;
    public bool ExplorerMenu
    {
        get => _explorerMenu;
        set
        {
            if (value == _explorerMenu) return;
            try
            {
                if (value) ExplorerIntegration.Enable();
                else ExplorerIntegration.Disable();
                _explorerMenu = value;
            }
            catch (Exception ex)
            {
                _ = _main.ShowError("Не удалось изменить меню Проводника", ex.Message);
            }
            OnPropertyChanged();
            OnPropertyChanged(nameof(ExplorerMenuNote));
        }
    }

    public string ExplorerMenuNote => _explorerMenu && ExplorerIntegration.PointsElsewhere
        ? "Пункт меню запускает другую копию программы — выключите и включите его снова."
        : "Пункт «Анализировать в «Простор»» в меню папок и дисков. В Windows 11 — в разделе «Показать дополнительные параметры».";

    // ------------------------------------------------------------------ updates

    public bool CheckUpdates
    {
        get => S.CheckUpdates;
        set
        {
            S.CheckUpdates = value;
            S.Save();
            OnPropertyChanged();
        }
    }

    public string UpdateRepo
    {
        get => S.UpdateRepo;
        set
        {
            S.UpdateRepo = (value ?? "").Trim();
            S.Save();
            OnPropertyChanged();
        }
    }

    public string VersionText => $"Версия {Updater.CurrentVersionText}" + (Updater.IsLiteBuild ? " · компактная сборка (нужен .NET 10)" : "");
    public string DataFolder => AppSettings.DataFolder;
}
