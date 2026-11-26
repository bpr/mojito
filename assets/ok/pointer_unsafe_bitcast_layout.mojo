# A read or write through a `Pointer.unsafe_bitcast` retype to a pointee of
# another layout reads or writes the bytes the native layout puts at the
# address, as upstream's `pop.pointer.bitcast` and a typed load or store do:
# a struct's words read as bytes, a byte buffer's bytes read and written as
# wider values and as a struct, a `Bool` byte read as `UInt8`, a pointer read
# as its `Int` address, and a struct's bytes printed as text.
from std.memory.alloc import unsafe_alloc


@fieldwise_init
struct Pair(ImplicitlyCopyable):
    var a: Int
    var b: Int


@fieldwise_init
struct Count(Movable):
    var value: Int


@fieldwise_init
struct Tagged(Movable):
    var tag: Int32
    var value: Int


def bump(mut x: Int):
    x += 5


def bool_bytes():
    var b = True
    print(Pointer(to=b).unsafe_bitcast[UInt8]()[])
    var u: UInt8 = 0
    Pointer(to=u).unsafe_bitcast[Bool]()[] = True
    print(u)


def pointer_words():
    var x = 5
    var p = Pointer(to=x)
    print(Pointer(to=p).unsafe_bitcast[Int]()[] == Int(p))


def struct_bytes():
    var s = Pair(0x0102, 0x7F)
    var bp = Pointer(to=s).unsafe_bitcast[Byte]()
    print(bp[], bp[unsafe_offset=1], bp[unsafe_offset=8])
    bp[unsafe_offset=9] = 1
    print(s.b)
    var end = bp.unsafe_offset(16)
    print(end - bp, end == bp, bp.unsafe_offset(8).unsafe_bitcast[Int]()[])


def buffer_words():
    var buf = unsafe_alloc[Byte](16)
    for i in range(16):
        buf[unsafe_offset=i] = UInt8(i)
    var p = buf.unsafe_bitcast[Int]()
    print(p[])
    print(p[unsafe_offset=1])
    p[] = -1
    print(buf[unsafe_offset=0], buf[unsafe_offset=7], buf[unsafe_offset=8])
    print(Int(p) - Int(buf))
    buf.unsafe_free()


def buffer_struct():
    var buf = unsafe_alloc[Byte](16)
    buf.unsafe_bitcast[Count]().unsafe_write(Count(1))
    var rc = buf.unsafe_bitcast[Count]()
    rc[].value += 1
    print(rc[].value)
    print(buf[unsafe_offset=0], buf[unsafe_offset=1])
    buf.unsafe_free()


def views_of_views():
    var buf = unsafe_alloc[Int32](4)
    for i in range(4):
        buf[unsafe_offset=i] = Int32(i + 1)
    var w = buf.unsafe_bitcast[Int]()
    print(w[unsafe_offset=1])
    w.unsafe_offset(1).unsafe_bitcast[Byte]()[unsafe_offset=4] = 9
    print(buf[unsafe_offset=3])
    buf.unsafe_free()


def mutable_views():
    var buf = unsafe_alloc[Byte](32)
    for i in range(32):
        buf[unsafe_offset=i] = 0
    var w = buf.unsafe_bitcast[Int]()
    bump(w[unsafe_offset=1])
    print(buf[unsafe_offset=8])
    var c = buf.unsafe_offset(16).unsafe_bitcast[Tagged]()
    c[].value = 300
    bump(c[].value)
    print(
        Int(Pointer(to=c[].value)) - Int(buf),
        c[].value,
        buf[unsafe_offset=24],
        buf[unsafe_offset=25],
    )
    buf.unsafe_free()


def struct_text():
    var s = Pair(0x6F6C6C6568, 0)
    var bp = Pointer(to=s).unsafe_bitcast[Byte]()
    var span = Span[Byte, origin_of(s)](unsafe_ptr=bp, length=5)
    print(StringSpan[origin_of(s)](unsafe_from_utf8=span))


def main():
    bool_bytes()
    pointer_words()
    struct_bytes()
    buffer_words()
    buffer_struct()
    views_of_views()
    mutable_views()
    struct_text()
