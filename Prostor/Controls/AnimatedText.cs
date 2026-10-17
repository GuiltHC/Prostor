using System.Windows;
using System.Windows.Controls;
using System.Windows.Media.Animation;
using Prostor.Core;

namespace Prostor.Controls;

public enum NumberFormat { Number, Size, Percent }

/// <summary>Text that "counts up" to a new number (sizes, counts, percents).</summary>
public sealed class AnimatedText : TextBlock
{
    public static readonly DependencyProperty ValueProperty = DependencyProperty.Register(nameof(Value), typeof(double), typeof(AnimatedText),
        new PropertyMetadata(double.NaN, (d, e) => ((AnimatedText)d).Animate((double)e.OldValue, (double)e.NewValue)));
    public static readonly DependencyProperty FormatProperty = DependencyProperty.Register(nameof(Format), typeof(NumberFormat), typeof(AnimatedText),
        new PropertyMetadata(NumberFormat.Number, (d, _) => ((AnimatedText)d).Render()));
    public static readonly DependencyProperty PlaceholderProperty = DependencyProperty.Register(nameof(Placeholder), typeof(string), typeof(AnimatedText),
        new PropertyMetadata("—", (d, _) => ((AnimatedText)d).Render()));

    private static readonly DependencyProperty ShownProperty = DependencyProperty.Register("Shown", typeof(double), typeof(AnimatedText),
        new PropertyMetadata(0.0, (d, _) => ((AnimatedText)d).Render()));

    /// <summary>NaN or a negative value shows <see cref="Placeholder"/>.</summary>
    public double Value { get => (double)GetValue(ValueProperty); set => SetValue(ValueProperty, value); }
    public NumberFormat Format { get => (NumberFormat)GetValue(FormatProperty); set => SetValue(FormatProperty, value); }
    public string Placeholder { get => (string)GetValue(PlaceholderProperty); set => SetValue(PlaceholderProperty, value); }

    public AnimatedText() => Render();

    private void Animate(double from, double to)
    {
        if (double.IsNaN(to) || to < 0)
        {
            BeginAnimation(ShownProperty, null);
            Render();
            return;
        }
        if (double.IsNaN(from) || from < 0) from = 0;
        var anim = new DoubleAnimation(from, to, TimeSpan.FromMilliseconds(to == from ? 1 : 750))
        {
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut }
        };
        BeginAnimation(ShownProperty, anim);
    }

    private void Render()
    {
        double v = Value;
        if (double.IsNaN(v) || v < 0)
        {
            Text = Placeholder;
            return;
        }
        double shown = (double)GetValue(ShownProperty);
        Text = Format switch
        {
            NumberFormat.Size => Fmt.Size((long)shown),
            NumberFormat.Percent => Math.Round(shown * 100) + "%",
            _ => Fmt.Number((long)Math.Round(shown))
        };
    }
}
