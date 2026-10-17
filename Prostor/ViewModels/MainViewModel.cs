using System.Collections.ObjectModel;
using System.Windows.Input;
using Prostor.Core;
using Prostor.Models;
using Prostor.Services;

namespace Prostor.ViewModels;

public sealed class MainViewModel : ObservableObject
{
    public static MainViewModel Instance { get; private set; }

    public ObservableCollection<NavItem> NavItems { get; }
    public ObservableCollection<NavItem> FooterNavItems { get; }

    public DashboardViewModel Dashboard { get; }
    public ProgramsViewModel Programs { get; }
    public AnalyzerViewModel Analyzer { get; }
    public CleanupViewModel Cleanup { get; }
    public StartupViewModel Startup { get; }
    public JournalViewModel Journal { get; }
    public SettingsViewModel Settings { get; }

    public bool IsAdmin => ShellHelper.IsAdmin;
    public string ModeText => IsAdmin ? "Режим администратора" : "Обычный режим";

    public ICommand ToggleThemeCommand { get; }
    public ICommand RestartAsAdminCommand { get; }
    public ICommand InstallUpdateCommand { get; }
    public ICommand DismissUpdateCommand { get; }
    public ICommand OpenSettingsCommand { get; }

    public MainViewModel()
    {
        Instance = this;
        NavItems = new ObservableCollection<NavItem>
        {
            new() { Key = "dashboard", Title = "Обзор", Glyph = "", Hotkey = "Ctrl+1" },
            new() { Key = "programs", Title = "Программы", Glyph = "", Hotkey = "Ctrl+2" },
            new() { Key = "analyzer", Title = "Анализ диска", Glyph = "", Hotkey = "Ctrl+3" },
            new() { Key = "cleanup", Title = "Очистка", Glyph = "", Hotkey = "Ctrl+4" },
            new() { Key = "startup", Title = "Автозагрузка", Glyph = "", Hotkey = "Ctrl+5" },
        };
        FooterNavItems = new ObservableCollection<NavItem>
        {
            new() { Key = "journal", Title = "Журнал", Glyph = "", Hotkey = "Ctrl+6" },
            new() { Key = "settings", Title = "Настройки", Glyph = "", Hotkey = "Ctrl+," },
        };

        Programs = new ProgramsViewModel(this);
        Analyzer = new AnalyzerViewModel(this);
        Cleanup = new CleanupViewModel(this);
        Startup = new StartupViewModel(this);
        Journal = new JournalViewModel(this);
        Settings = new SettingsViewModel(this);
        Dashboard = new DashboardViewModel(this);

        ToggleThemeCommand = new RelayCommand(() =>
        {
            AppSettings.Current.Theme = ThemeManager.IsDark ? "light" : "dark";
            AppSettings.Current.Save();
            ThemeManager.Apply();
            Settings.RaiseTheme();
        });
        ThemeManager.Changed += () =>
        {
            OnPropertyChanged(nameof(IsDark));
            OnPropertyChanged(nameof(ThemeText));
            OnPropertyChanged(nameof(ThemeGlyph));
        };
        RestartAsAdminCommand = new RelayCommand(async () =>
        {
            if (Analyzer.IsScanning || Cleanup.IsCleaning || Programs.IsBusy)
            {
                var ok = await ShowDialog(new DialogViewModel
                {
                    Title = "Перезапустить сейчас?",
                    Message = "Текущая операция будет прервана.",
                    Glyph = "",
                    PrimaryText = "Перезапустить",
                    SecondaryText = "Отмена"
                });
                if (!ok) return;
            }
            if (!ShellHelper.RestartAsAdmin(CurrentKey)) ShowToast("Запуск с правами администратора отменён", "warning");
        });
        InstallUpdateCommand = new RelayCommand(async () => await InstallUpdateAsync(), () => Update != null);
        DismissUpdateCommand = new RelayCommand(() =>
        {
            if (Update == null) return;
            AppSettings.Current.SkippedVersion = Update.Tag;
            AppSettings.Current.Save();
            Update = null;
        });

        OpenSettingsCommand = new RelayCommand(() => Navigate("settings"));

        _selectedNav = NavItems[0];
    }

    private NavItem _selectedNav;
    public NavItem SelectedNav
    {
        get => _selectedNav;
        set
        {
            if (value == null || !Set(ref _selectedNav, value)) return;
            OnPropertyChanged(nameof(CurrentKey));
            OnPropertyChanged(nameof(SelectedMainNav));
            OnPropertyChanged(nameof(SelectedFooterNav));
            if (value.Key == "journal") Journal.Reload();
            if (value.Key == "settings") Settings.Refresh();
        }
    }

    public string CurrentKey => _selectedNav?.Key;

    /// <summary>Selection of the upper navigation list (null while a footer page is open).</summary>
    public NavItem SelectedMainNav
    {
        get => NavItems.Contains(_selectedNav) ? _selectedNav : null;
        set { if (value != null) SelectedNav = value; }
    }

    public NavItem SelectedFooterNav
    {
        get => FooterNavItems.Contains(_selectedNav) ? _selectedNav : null;
        set { if (value != null) SelectedNav = value; }
    }

    public void Navigate(string key)
    {
        var item = NavItems.Concat(FooterNavItems).FirstOrDefault(n => n.Key == key);
        if (item != null) SelectedNav = item;
    }

    public bool IsDark => ThemeManager.IsDark;
    public string ThemeText => ThemeManager.IsDark ? "Светлая тема" : "Тёмная тема";
    public string ThemeGlyph => ThemeManager.IsDark ? "" : "";

    // ------------------------------------------------------------------ hotkeys

    public void RefreshCurrent()
    {
        switch (CurrentKey)
        {
            case "dashboard": Dashboard.RefreshCommand.Execute(null); break;
            case "programs": if (!Programs.IsLoading) _ = Programs.LoadAsync(); break;
            case "analyzer": if (Analyzer.RescanCommand.CanExecute(null)) Analyzer.RescanCommand.Execute(null); break;
            case "cleanup": if (Cleanup.ScanCommand.CanExecute(null)) Cleanup.ScanCommand.Execute(null); break;
            case "startup": Startup.RefreshCommand.Execute(null); break;
            case "journal": Journal.Reload(); break;
        }
    }

    public void DeleteInCurrent()
    {
        switch (CurrentKey)
        {
            case "programs":
                var cmd = Programs.CheckedCount > 0 ? Programs.UninstallCheckedCommand : Programs.UninstallCommand;
                if (cmd.CanExecute(null)) cmd.Execute(null);
                break;
            case "analyzer":
                if (Analyzer.DeleteCommand.CanExecute(null)) Analyzer.DeleteCommand.Execute(null);
                break;
        }
    }

    public void GoUpInCurrent()
    {
        if (CurrentKey == "analyzer" && Analyzer.UpCommand.CanExecute(null)) Analyzer.UpCommand.Execute(null);
    }

    // ------------------------------------------------------------------ dialogs & toasts

    private DialogViewModel _dialog;
    public DialogViewModel Dialog { get => _dialog; set => Set(ref _dialog, value); }

    public Task<bool> ShowDialog(DialogViewModel dialog)
    {
        Dialog?.Close(false);
        Dialog = dialog;
        return dialog.Result;
    }

    public Task ShowError(string title, string message) => ShowDialog(new DialogViewModel
    {
        Title = title,
        Message = message,
        Glyph = "",
        Kind = "danger",
        PrimaryText = "Понятно"
    });

    private ToastInfo _toast;
    public ToastInfo Toast { get => _toast; private set => Set(ref _toast, value); }
    private CancellationTokenSource _toastCts;

    public async void ShowToast(string text, string kind = "success")
    {
        _toastCts?.Cancel();
        var cts = _toastCts = new CancellationTokenSource();
        Toast = new ToastInfo
        {
            Text = text,
            Kind = kind,
            Glyph = kind switch { "warning" => "", "danger" => "", "info" => "", _ => "" }
        };
        try
        {
            await Task.Delay(4200, cts.Token);
            Toast = null;
        }
        catch (TaskCanceledException) { }
    }

    // ------------------------------------------------------------------ updates

    private ReleaseInfo _update;
    public ReleaseInfo Update
    {
        get => _update;
        private set
        {
            if (Set(ref _update, value)) OnPropertyChanged(nameof(UpdateText));
        }
    }

    public string UpdateText => _update == null ? null : $"Доступна версия {_update.Version.ToString(3)}";

    public async Task CheckForUpdatesAsync(bool manual)
    {
        var s = AppSettings.Current;
        if (string.IsNullOrWhiteSpace(s.UpdateRepo))
        {
            if (manual) ShowToast("Адрес репозитория для обновлений не задан", "warning");
            return;
        }
        try
        {
            var r = await Updater.CheckAsync(s.UpdateRepo);
            if (r == null)
            {
                if (manual) ShowToast($"У вас последняя версия — {Updater.CurrentVersionText}", "info");
                return;
            }
            if (!manual && r.Tag == s.SkippedVersion) return;
            Update = r;
            if (manual) await InstallUpdateAsync();
        }
        catch (Exception ex)
        {
            if (manual) await ShowError("Не удалось проверить обновления", ex.Message);
        }
    }

    private async Task InstallUpdateAsync()
    {
        var r = Update;
        if (r == null) return;
        var ask = new DialogViewModel
        {
            Title = $"Обновить до версии {r.Version.ToString(3)}?",
            Message = (string.IsNullOrWhiteSpace(r.Notes) ? "" : r.Notes.Trim() + "\n\n") +
                      "Программа скачает новую версию, заменит себя и перезапустится.",
            Glyph = "",
            PrimaryText = "Обновить",
            SecondaryText = "Позже",
            TertiaryText = r.PageUrl != null ? "Страница выпуска" : null
        };
        await ShowDialog(ask);
        if (ask.Choice == "tertiary")
        {
            ShellHelper.Open(r.PageUrl);
            return;
        }
        if (ask.Choice != "primary") return;

        var busy = new DialogViewModel
        {
            Title = "Скачиваем обновление…", Glyph = "", IsBusy = true, PrimaryText = null,
            ShowProgress = true, IsIndeterminate = false
        };
        _ = ShowDialog(busy);
        try
        {
            await Updater.InstallAsync(r, new Progress<double>(p =>
            {
                busy.Progress = p;
                busy.ProgressText = Fmt.Percent(p);
            }), CancellationToken.None);
        }
        catch (Exception ex)
        {
            busy.Close(false);
            await ShowError("Не удалось обновить программу", ex.Message);
        }
    }

    // ------------------------------------------------------------------ startup

    public async Task InitializeAsync()
    {
        _ = Task.Run(DiskHistory.Record).ContinueWith(_ => System.Windows.Application.Current.Dispatcher.InvokeAsync(Dashboard.RefreshHistory));
        Dashboard.Refresh();
        var programs = Programs.LoadAsync();
        var startup = Startup.LoadAsync();
        var cleanup = Cleanup.ScanAsync();
        if (AppSettings.Current.CheckUpdates) _ = CheckForUpdatesAsync(false);
        await Task.WhenAll(programs, startup, cleanup);
    }

    public void UpdateBadges()
    {
        NavItems[1].Badge = Programs.Apps.Count > 0 ? Programs.Apps.Count.ToString() : null;
        NavItems[3].Badge = Cleanup.TotalFound > 0 ? Fmt.Size(Cleanup.TotalFound) : null;
        NavItems[4].Badge = Startup.Items.Count > 0 ? $"{Startup.EnabledCount}" : null;
    }
}
