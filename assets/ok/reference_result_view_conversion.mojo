# A reference result converts to a `StringSpan` in place, as the pin does: the
# view's `ref [origin]` source binds the referent an accessor or a free `ref`
# function returns, and the view keeps that referent's owner alive while the
# call or binding that consumes it lives.
@fieldwise_init
struct P(Copyable, Movable):
    var name: String

    def name_ref(ref self) -> ref [self.name] String:
        return self.name


def pick(ref a: String) -> ref [a] String:
    return a


def width(s: StringSpan) -> Int:
    return s.byte_length()


def main():
    var p = P("wxyz")
    print(width(p.name_ref()))
    var out = String("a")
    out += p.name_ref()
    print(out)
    var t = String("hello")
    print(width(pick(t)))
    out += pick(t)
    print(out)
    var view: StringSpan[origin_of(p.name)] = p.name_ref()
    print(view)
    var xs = [String("ab"), String("cde")]
    var element: StringSpan[origin_of(xs)] = xs[1]
    print(element)
    var q = P("ab")
    for _ in range(2):
        q.name += "c"
        print(width(q.name_ref()))
    print(p.name, t)
