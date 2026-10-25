# An element read through a view (`String(span[0])`, `span[0].copy()`, a
# field of `rest[0]`) names the view's own origin, so the view stays usable
# afterwards, and the owner may change once the view's last use has passed.
@fieldwise_init
struct D(Copyable, Movable):
    var v: Int


def main() raises:
    var items = List[String]()
    items.append("x")
    var span: Span[String, _] = items
    var copied = span[0].copy()
    var converted = String(span[0])
    print(converted, copied, len(span))
    items.append("z")
    print(len(items))

    var xs = List[D]()
    xs.append(D(1))
    xs.append(D(2))
    var view: Span[D, _] = xs
    var first = view[0].v
    print(first, len(view))
    var rest = xs[1:]
    print(rest[0].v, len(rest))
    var [*all] = xs
    print(all[0].v, len(all))
