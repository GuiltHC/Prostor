using System.ComponentModel;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Shell;
using Prostor.Core;
using Prostor.ViewModels;

namespace Prostor;

public partial class MainWindow : Window
{
    private readonly MainViewModel _vm;

    public string[] StartupArgs { get; init; } = Array.Empty<string>();

    public MainWindow()
    {
        InitializeComponent();
        _vm = new MainViewModel();
        DataContext = _vm;
        RestoreWindowBounds();

        _vm.PropertyChanged += OnVmPropertyChanged;
        SourceInitialized += (_, _) =>
        {
            HwndSource.FromHwnd(new WindowInteropHelper(this).Handle)?.AddHook(WndProc);
            ApplyWindowFrame();
        };
        ThemeManager.Changed += ApplyWindowFrame;
        StateChanged += (_, _) => UpdateMaximizedState();
        Loaded += async (_, _) =>
        {
            UpdateMaximizedState();
            if (!HandleArgs(StartupArgs, activate: false) && AppSettings.Current.LastPage is { } last && last != "dashboard")
                _vm.Navigate(last);
            await _vm.InitializeAsync();
        };
        Closing += (_, _) => SaveWindowBounds();
        ToastHost.IsVisibleChanged += (_, e) =>
        {
            if (e.NewValue is true) AnimateIn(ToastHost, 0, 14);
        };
        DialogLayer.IsVisibleChanged += (_, e) =>
        {
            if (e.NewValue is true) AnimateDialog();
        };
        PreviewKeyDown += OnPreviewKeyDown;
        DragEnter += OnDragOver;
        DragOver += OnDragOver;
        DragLeave += (_, _) => DropOverlay.Visibility = Visibility.Collapsed;
        Drop += OnDrop;
    }

    /// <summary>
    /// Arguments from the command line or from a second launch: --page &lt;key&gt;, --scan &lt;path&gt;, --theme light|dark.
    /// Returns true when they chose a page.
    /// </summary>
    public bool HandleArgs(string[] args, bool activate)
    {
        bool navigated = false;
        for (int i = 0; i < args.Length; i++)
        {
            string next = i + 1 < args.Length ? args[i + 1] : null;
            switch (args[i])
            {
                case "--page" when next != null:
                    _vm.Navigate(next);
                    navigated = true;
                    i++;
                    break;
                case "--scan" when next != null:
                    _vm.Analyzer.StartScan(next.Trim('"'));
                    navigated = true;
                    i++;
                    break;
                case "--theme" when next != null:
                    AppSettings.Current.Theme = next == "light" ? "light" : next == "system" ? "system" : "dark";
                    ThemeManager.Apply();
                    i++;
                    break;
            }
        }
        if (activate) BringToFront();
        return navigated;
    }

    private void BringToFront()
    {
        if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;
        Activate();
        Topmost = true;
        Topmost = false;
        Focus();
    }

    private void OnVmPropertyChanged(object sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(MainViewModel.CurrentKey)) ShowPage(_vm.CurrentKey);
    }

    private FrameworkElement PageFor(string key) => key switch
    {
        "programs" => ProgramsPage,
        "analyzer" => AnalyzerPage,
        "cleanup" => CleanupPage,
        "startup" => StartupPage,
        "journal" => JournalPage,
        "settings" => SettingsPage,
        _ => DashboardPage
    };

    private void ShowPage(string key)
    {
        var target = PageFor(key);
        foreach (FrameworkElement page in Pages.Children)
            page.Visibility = page == target ? Visibility.Visible : Visibility.Collapsed;
        AnimateIn(target, 0, 10);
    }

    private static void AnimateIn(FrameworkElement element, double fromOpacity, double offsetY)
    {
        var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
        element.BeginAnimation(OpacityProperty, new DoubleAnimation(fromOpacity, 1, TimeSpan.FromMilliseconds(220)) { EasingFunction = ease });
        if (element.RenderTransform is not TranslateTransform tt || tt.IsFrozen)
        {
            tt = new TranslateTransform();
            element.RenderTransform = tt;
        }
        tt.BeginAnimation(TranslateTransform.YProperty, new DoubleAnimation(offsetY, 0, TimeSpan.FromMilliseconds(260)) { EasingFunction = ease });
    }

    private void AnimateDialog()
    {
        var ease = new CubicEase { EasingMode = EasingMode.EaseOut };
        DialogLayer.BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(160)));
        if (DialogCard.RenderTransform is not ScaleTransform st || st.IsFrozen)
        {
            st = new ScaleTransform();
            DialogCard.RenderTransform = st;
        }
        var anim = new DoubleAnimation(0.96, 1, TimeSpan.FromMilliseconds(200)) { EasingFunction = ease };
        st.BeginAnimation(ScaleTransform.ScaleXProperty, anim);
        st.BeginAnimation(ScaleTransform.ScaleYProperty, anim);
    }

    // ------------------------------------------------------------------ keyboard

    private void OnPreviewKeyDown(object sender, KeyEventArgs e)
    {
        if (_vm.Dialog is { } d)
        {
            if (e.Key == Key.Escape && !d.IsBusy && (d.SecondaryText != null || d.PrimaryText != null))
            {
                d.Close(false);
                e.Handled = true;
            }
            return;
        }

        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        var mods = Keyboard.Modifiers;
        bool inText = Keyboard.FocusedElement is TextBox;

        if (mods == ModifierKeys.Control)
        {
            int index = key switch
            {
                Key.D1 or Key.NumPad1 => 0, Key.D2 or Key.NumPad2 => 1, Key.D3 or Key.NumPad3 => 2,
                Key.D4 or Key.NumPad4 => 3, Key.D5 or Key.NumPad5 => 4, _ => -1
            };
            if (index >= 0)
            {
                _vm.SelectedNav = _vm.NavItems[index];
                e.Handled = true;
                return;
            }
            if (key is Key.D6 or Key.NumPad6)
            {
                _vm.Navigate("journal");
                e.Handled = true;
                return;
            }
            if (key == Key.OemComma)
            {
                _vm.Navigate("settings");
                e.Handled = true;
                return;
            }
            if (key == Key.F)
            {
                if (FindSearchBox(PageFor(_vm.CurrentKey)) is { } box)
                {
                    box.Focus();
                    box.SelectAll();
                }
                e.Handled = true;
                return;
            }
        }

        if (key == Key.F5 && mods == ModifierKeys.None)
        {
            _vm.RefreshCurrent();
            e.Handled = true;
            return;
        }
        if (inText)
        {
            if (key == Key.Escape && Keyboard.FocusedElement is TextBox tb)
            {
                if (tb.Text.Length > 0) tb.Clear();
                else Keyboard.ClearFocus();
                e.Handled = true;
            }
            return;
        }
        if (key == Key.Delete && mods == ModifierKeys.None)
        {
            _vm.DeleteInCurrent();
            e.Handled = true;
        }
        else if ((key == Key.Back && mods == ModifierKeys.None) || (key == Key.Left && mods == ModifierKeys.Alt))
        {
            _vm.GoUpInCurrent();
            e.Handled = true;
        }
    }

    private static TextBox FindSearchBox(DependencyObject root)
    {
        if (root == null) return null;
        int n = VisualTreeHelper.GetChildrenCount(root);
        for (int i = 0; i < n; i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is TextBox { IsVisible: true } t && t.Style == Application.Current.TryFindResource("SearchBox")) return t;
            var found = FindSearchBox(child);
            if (found != null) return found;
        }
        return null;
    }

    // ------------------------------------------------------------------ drag & drop

    private static string DroppedFolder(DragEventArgs e)
    {
        if (!e.Data.GetDataPresent(DataFormats.FileDrop) || e.Data.GetData(DataFormats.FileDrop) is not string[] { Length: > 0 } paths)
            return null;
        string p = paths[0];
        if (Directory.Exists(p)) return p;
        return File.Exists(p) ? Path.GetDirectoryName(p) : null;
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        string folder = _vm.Dialog == null ? DroppedFolder(e) : null;
        e.Effects = folder != null ? DragDropEffects.Link : DragDropEffects.None;
        e.Handled = true;
        if (folder == null)
        {
            DropOverlay.Visibility = Visibility.Collapsed;
            return;
        }
        string name = Path.GetFileName(folder.TrimEnd('\\'));
        DropText.Text = $"Отпустите, чтобы проанализировать «{(string.IsNullOrEmpty(name) ? folder : name)}»";
        DropOverlay.Visibility = Visibility.Visible;
    }

    private void OnDrop(object sender, DragEventArgs e)
    {
        DropOverlay.Visibility = Visibility.Collapsed;
        string folder = _vm.Dialog == null ? DroppedFolder(e) : null;
        if (folder == null) return;
        e.Handled = true;
        _vm.Analyzer.StartScan(folder);
        Activate();
    }

    // ------------------------------------------------------------------ window chrome

    private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        if (msg == Native.WM_SETTINGCHANGE && lParam != IntPtr.Zero &&
            System.Runtime.InteropServices.Marshal.PtrToStringUni(lParam) == "ImmersiveColorSet")
            Dispatcher.InvokeAsync(ThemeManager.OnSystemSettingsChanged);
        else if (msg == Native.WM_DWMCOLORIZATIONCOLORCHANGED && AppSettings.Current.UseSystemAccent)
            Dispatcher.InvokeAsync(ThemeManager.OnSystemSettingsChanged);
        return IntPtr.Zero;
    }

    private void ApplyWindowFrame()
    {
        var hwnd = new WindowInteropHelper(this).Handle;
        if (hwnd == IntPtr.Zero) return;
        int dark = ThemeManager.IsDark ? 1 : 0;
        Native.DwmSetWindowAttribute(hwnd, Native.DWMWA_USE_IMMERSIVE_DARK_MODE, ref dark, sizeof(int));
        int round = 2; // DWMWCP_ROUND
        Native.DwmSetWindowAttribute(hwnd, Native.DWMWA_WINDOW_CORNER_PREFERENCE, ref round, sizeof(int));

        var source = HwndSource.FromHwnd(hwnd);
        bool mica = ThemeManager.MicaActive;
        if (mica)
        {
            Background = Brushes.Transparent;
            if (source?.CompositionTarget != null) source.CompositionTarget.BackgroundColor = Colors.Transparent;
            WindowChrome.SetWindowChrome(this, NewChrome(new Thickness(-1)));
            var margins = new Native.MARGINS { Left = -1, Right = -1, Top = -1, Bottom = -1 };
            Native.DwmExtendFrameIntoClientArea(hwnd, ref margins);
        }
        else
        {
            SetResourceReference(BackgroundProperty, "Brush.Bg");
            if (source?.CompositionTarget != null && TryFindResource("Brush.Bg") is SolidColorBrush bg)
                source.CompositionTarget.BackgroundColor = bg.Color;
            WindowChrome.SetWindowChrome(this, NewChrome(new Thickness(0, 0, 0, 1)));
        }
        if (ThemeManager.MicaSupported)
        {
            int backdrop = mica ? Native.DWMSBT_MAINWINDOW : Native.DWMSBT_NONE;
            Native.DwmSetWindowAttribute(hwnd, Native.DWMWA_SYSTEMBACKDROP_TYPE, ref backdrop, sizeof(int));
        }
        if (TryFindResource("Brush.Border") is SolidColorBrush b)
        {
            int color = b.Color.R | (b.Color.G << 8) | (b.Color.B << 16);
            Native.DwmSetWindowAttribute(hwnd, Native.DWMWA_BORDER_COLOR, ref color, sizeof(int));
        }
    }

    private static WindowChrome NewChrome(Thickness glass) => new()
    {
        CaptionHeight = 40,
        ResizeBorderThickness = new Thickness(6),
        GlassFrameThickness = glass,
        CornerRadius = new CornerRadius(0),
        UseAeroCaptionButtons = false
    };

    private void UpdateMaximizedState()
    {
        if (WindowState == WindowState.Maximized)
        {
            var t = SystemParameters.WindowResizeBorderThickness;
            RootGrid.Margin = new Thickness(t.Left + 2, t.Top + 2, t.Right + 2, t.Bottom + 2);
            MaxButton.SetValue(UI.GlyphProperty, "");
            MaxButton.ToolTip = "Восстановить";
        }
        else
        {
            RootGrid.Margin = new Thickness(0);
            MaxButton.SetValue(UI.GlyphProperty, "");
            MaxButton.ToolTip = "Развернуть";
        }
    }

    private void RestoreWindowBounds()
    {
        var b = AppSettings.Current.Window;
        if (b == null || b.Width < MinWidth || b.Height < MinHeight) return;
        var screen = new Rect(SystemParameters.VirtualScreenLeft, SystemParameters.VirtualScreenTop,
            SystemParameters.VirtualScreenWidth, SystemParameters.VirtualScreenHeight);
        var rect = new Rect(b.Left, b.Top, b.Width, b.Height);
        var visible = Rect.Intersect(screen, rect);
        if (visible.IsEmpty || visible.Width < 200 || visible.Height < 120) return;
        WindowStartupLocation = WindowStartupLocation.Manual;
        Left = b.Left;
        Top = b.Top;
        Width = Math.Min(b.Width, screen.Width);
        Height = Math.Min(b.Height, screen.Height);
        if (b.Maximized) WindowState = WindowState.Maximized;
    }

    private void SaveWindowBounds()
    {
        var r = WindowState == WindowState.Normal ? new Rect(Left, Top, ActualWidth, ActualHeight) : RestoreBounds;
        if (r.IsEmpty || r.Width <= 0) return;
        var s = AppSettings.Current;
        s.Window = new WindowBounds
        {
            Left = r.Left, Top = r.Top, Width = r.Width, Height = r.Height,
            Maximized = WindowState == WindowState.Maximized
        };
        s.LastPage = _vm.CurrentKey;
        s.Save();
    }

    private void Minimize_Click(object sender, RoutedEventArgs e) => WindowState = WindowState.Minimized;

    private void Maximize_Click(object sender, RoutedEventArgs e) =>
        WindowState = WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;

    private void Close_Click(object sender, RoutedEventArgs e) => Close();
}
