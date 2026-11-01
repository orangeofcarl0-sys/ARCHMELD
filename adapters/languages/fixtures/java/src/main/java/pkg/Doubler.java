package pkg;

public final class Doubler implements Transformer {
    @Override
    public int apply(int value) {
        return value + value;
    }
}
