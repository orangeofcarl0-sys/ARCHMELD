package pkg;

import java.lang.reflect.Method;
import java.util.function.IntUnaryOperator;

public final class App {
    public static int run() {
        Transformer transformer = new Doubler();
        IntUnaryOperator op = Util::helper;
        return Util.helper(1) + Util.helper("xy") + op.applyAsInt(2)
                + transformer.apply(3) + Other.helper(9);
    }

    public static Object reflect() throws Exception {
        Method method = Class.forName("pkg.Util").getMethod("helper", int.class);
        return method.invoke(null, 5);
    }
}
