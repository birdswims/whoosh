using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Whoosh;

public sealed partial class ActivityWindow : UserControl
{
    public event EventHandler? DismissRequested;

    public ActivityWindow()
    {
        InitializeComponent();
        Root.DataContext = App.Model;
    }

    void Back_Click(object sender, RoutedEventArgs e) => DismissRequested?.Invoke(this, EventArgs.Empty);

    void Clear_Click(object sender, RoutedEventArgs e) => App.Model.ClearActivity();

    void Show_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: ActivityItem item })
        {
            App.Model.ShowActivity(item);
        }
    }
}
