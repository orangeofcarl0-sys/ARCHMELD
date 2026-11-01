using System;

public delegate int Transformer(int value);

public static class Program
{
    public static int Main()
    {
        Func<int, int> f = Util.Helper;
        Transformer t = Util.Helper;
        Console.WriteLine(Util.Helper(1) + f(2) + t(3));
        return 0;
    }
}
