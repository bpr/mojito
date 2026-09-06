# Two `Named` views over one value coexist, bound or temporary: `Named`'s
# origin is `Origin[mut=False]`, so lending `w` to its `ref[Self.o]`
# constructor parameter is a shared read at the call and a shared loan
# afterwards (upstream's `Named[T, o: ImmOrigin]` prints the same). A read
# of `w` beside the views is fine; a write while a view lives is rejected.
from std.format._utils import Named

def main():
    var w = 5
    var a = Named("a", w)
    var b = Named("b", w)
    print(a, b)
    print(Named("c", w), Named("d", w))
    print(w)
