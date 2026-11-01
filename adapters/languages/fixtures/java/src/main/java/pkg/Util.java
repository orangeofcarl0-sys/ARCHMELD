package pkg;

@Marker
public final class Util {
    public static int helper(int value) {
        return value * 2;
    }

    public static int helper(String value) {
        return value.length();
    }
}
