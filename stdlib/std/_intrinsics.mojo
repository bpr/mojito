# The Mojo bodies behind builtin scalar operations. Upstream implements these
# on `Int` itself; Mojito's `Int` is a compiler builtin with no struct to hang
# them on, so both backends call these free functions by symbol instead of
# carrying a hand-written Rust or LLVM implementation of the same algorithm.
#
# `std.prelude` imports the module so every linked program carries the bodies;
# the names stay underscored and out of `PRELUDE_EXPORTS`, so user code never
# sees them. Module-level names bind in source order, so a helper is defined
# before the functions that call it.


# `x ** y` on `Int`/`UInt`: wrapping square-and-multiply. The caller has
# already guarded the exponent to `0 ..= UInt32.MAX` (native trap category 2,
# the VM's `'**' exponent` type error), so the loop only sees a non-negative
# count. Wrapping i64 multiplication is bit-identical for `Int` and `UInt`, so
# the one body serves both: the unsigned caller reinterprets the bits.
def _pow_int(base: Int, exponent: Int) -> Int:
    var accumulator = 1
    var factor = base
    var remaining = exponent
    while remaining != 0:
        if remaining & 1 != 0:
            accumulator = accumulator * factor
        factor = factor * factor
        remaining = remaining >> 1
    return accumulator


# The digits of `value` at `out[at ...]`, returning how many were written.
# Counting first lets the fill run right to left, so there is no reversal.
def _uint_digits_at(value: UInt, out: UnsafePointer[Byte], at: Int) -> Int:
    var ten = UInt(10)
    var count = 1
    var scan = value
    while scan >= ten:
        scan = scan // ten
        count += 1
    var remaining = value
    var index = at + count
    while index > at:
        index -= 1
        var next = remaining // ten
        out[index] = Byte(Int(remaining - next * ten) + 48)
        remaining = next
    return count


# The decimal text of an integer, written as bytes at `out[0 ..< returned]`
# with no NUL terminator; `out` must hold at least 21 bytes (20 digits plus a
# sign). Upstream writes `Int` through `Int.write_to`; Mojito's `Int` is a
# builtin with no `write_to`, so the shared contract is this buffer instead of
# a `Writer` — the VM fills a heap scratch buffer, the native backend its
# per-function scratch alloca, and both read the same bytes back.
def _int_digits(value: Int, out: UnsafePointer[Byte]) -> Int:
    if value < 0:
        out[0] = Byte(45)
        # `0 - value` wraps for `Int.MIN`; read unsigned those bits are the
        # magnitude, so one path serves every negative.
        return 1 + _uint_digits_at(UInt(0) - UInt(value), out, 1)
    return _uint_digits_at(UInt(value), out, 0)


# `_int_digits` for an unsigned value, whose magnitude has no `Int` reading.
def _uint_digits(value: UInt, out: UnsafePointer[Byte]) -> Int:
    return _uint_digits_at(value, out, 0)
