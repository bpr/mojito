# A collection element converts to a `StringSpan` argument in place, as the
# pin does: the view's `ref [origin]` source binds the element of a `String`
# collector or the referent `List.__getitem__` returns, and the view keeps the
# container alive until the call that consumes it returns.
@fieldwise_init
struct Named(Copyable, Movable):
    var name: String


def width(s: StringSpan) -> Int:
    return s.byte_length()


def join(var *parts: String) -> String:
    var out = String("")
    for i in range(len(parts)):
        out += parts[i]
    return out


def first(*parts: String) -> String:
    var out = String("<")
    out += parts[0]
    return out


def main():
    print(join("x", "y"))
    print(first("a", "b"))
    var xs: List[String] = ["p", "qq"]
    print(width(xs[1]))
    var t = String("t")
    t += xs[0]
    xs.append("r")
    print(t, len(xs))
    var ns: List[Named] = [Named("ab"), Named("cde")]
    var u = String("")
    u += ns[1].name
    print(u, width(ns[0].name))
