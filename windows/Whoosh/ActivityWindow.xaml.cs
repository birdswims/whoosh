using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace Whoosh;

public sealed partial class ActivityWindow : Window
{
    public ActivityWindow()
    {
        InitializeComponent();
        Root.DataContext = App.Model;
        WindowChrome.KeepAtLeast(this, 420, 420);
        WindowChrome.Place(this, 480, 560);
    }

    void Clear_Click(object sender, RoutedEventArgs e) => App.Model.ClearActivity();

    void Show_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: ActivityItem item })
        {
            App.Model.ShowActivity(item);
        }
    }
}
