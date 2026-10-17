using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Threading;

namespace Prostor.Views;

/// <summary>Small self-closing notice in the bottom-right corner (used by the scheduled cleanup).</summary>
public sealed class NotificationWindow : Window
{
    public NotificationWindow(string title, string text)
    {
        WindowStyle = WindowStyle.None;
        AllowsTransparency = true;
        Background = Brushes.Transparent;
        ShowInTaskbar = false;
        Topmost = true;
        ShowActivated = false;
        ResizeMode = ResizeMode.NoResize;
        SizeToContent = SizeToContent.WidthAndHeight;
        FontFamily = new FontFamily("Segoe UI");

        var icon = new Border
        {
            Width = 36, Height = 36, CornerRadius = new CornerRadius(10), Margin = new Thickness(0, 0, 14, 0),
            VerticalAlignment = VerticalAlignment.Center,
            Child = new TextBlock
            {
                Text = "", FontFamily = new FontFamily("Segoe Fluent Icons, Segoe MDL2 Assets"), FontSize = 17,
                Foreground = Brushes.White, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center
            }
        };
        icon.SetResourceReference(Border.BackgroundProperty, "Brush.AccentGradient");

        var titleBlock = new TextBlock { Text = title, FontSize = 13, FontWeight = FontWeights.SemiBold };
        titleBlock.SetResourceReference(TextBlock.ForegroundProperty, "Brush.Text");
        var textBlock = new TextBlock { Text = text, FontSize = 13, Margin = new Thickness(0, 3, 0, 0), MaxWidth = 300, TextWrapping = TextWrapping.Wrap };
        textBlock.SetResourceReference(TextBlock.ForegroundProperty, "Brush.TextSecondary");

        var panel = new StackPanel { Orientation = Orientation.Horizontal };
        panel.Children.Add(icon);
        panel.Children.Add(new StackPanel { Children = { titleBlock, textBlock }, VerticalAlignment = VerticalAlignment.Center });

        var card = new Border
        {
            CornerRadius = new CornerRadius(12), BorderThickness = new Thickness(1), Padding = new Thickness(16, 14, 22, 14),
            Margin = new Thickness(16), Child = panel, Cursor = System.Windows.Input.Cursors.Hand,
            Effect = new System.Windows.Media.Effects.DropShadowEffect { BlurRadius = 24, ShadowDepth = 4, Opacity = 0.35 },
            ToolTip = "Открыть «Простор»"
        };
        card.SetResourceReference(Border.BackgroundProperty, "Brush.SurfaceHigh");
        card.SetResourceReference(Border.BorderBrushProperty, "Brush.BorderStrong");
        card.MouseLeftButtonUp += (_, _) =>
        {
            try { Process.Start(new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = true }); }
            catch { }
            Close();
        };
        Content = card;

        Loaded += (_, _) =>
        {
            var area = SystemParameters.WorkArea;
            Left = area.Right - ActualWidth;
            Top = area.Bottom - ActualHeight;
            BeginAnimation(OpacityProperty, new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(250)));
            var timer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(8) };
            timer.Tick += (_, _) =>
            {
                timer.Stop();
                var fade = new DoubleAnimation(1, 0, TimeSpan.FromMilliseconds(300));
                fade.Completed += (_, _) => Close();
                BeginAnimation(OpacityProperty, fade);
            };
            timer.Start();
        };
    }
}
