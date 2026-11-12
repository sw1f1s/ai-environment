namespace MissingDependencies;
public static class Broken {
    public static MissingType Missing(MissingType value) => value;
    public static object DynamicCall(dynamic target) => target.Unknown();
}
