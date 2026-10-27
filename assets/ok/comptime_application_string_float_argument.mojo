# A type or a parameter argument that computes a `String`, a `Float64`, or a
# `UInt` from a generic body's parameters: a call, a nested call, a
# construction, an operator, or a local `comptime` binding of one, in a body
# or a signature. The check carries the application, the elaborator runs it
# per instance, and a `String` result binds a `String` value parameter as the
# compile-time string it freezes to.

def pick(n: Int) -> String:
    if n > 1:
        return "big"
    return "small"

def half(n: Int) -> Float64:
    return Float64(n) / 2.0

def twice(x: Float64) -> Float64:
    return x * 2.0

def bump(x: UInt) -> UInt:
    return x + 1

def label[s: String]():
    print(s, s.byte_length())

def scale[x: Float64]():
    print(x)

def unsigned[x: UInt]():
    print(x)

def forward[s: String]():
    label[s]()

@fieldwise_init
struct Named[s: String](ImplicitlyCopyable):
    def show(self):
        print(Self.s)

@fieldwise_init
struct Scaled[x: Float64](ImplicitlyCopyable):
    def show(self):
        print(Self.x)

def scaled[n: Int]() -> Scaled[twice(half(n))]:
    return Scaled[twice(half(n))]()

def body[n: Int, m: UInt]():
    label[pick(n)]()
    label[pick(n) + "!"]()
    label[String(n)]()
    forward[pick(n)]()
    scale[half(n)]()
    scale[Float64(n) / 4.0]()
    unsigned[bump(m)]()
    comptime p = pick(n)
    label[p]()
    comptime h = half(n + 1)
    scale[h]()
    Named[pick(n)]().show()
    Scaled[half(n)]().show()

def main():
    body[3, 7]()
    body[1, 0]()
    scaled[5]().show()
    label[String("a")]()
    scale[half(9)]()
