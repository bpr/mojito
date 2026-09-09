# A self-hosted UTF-8 String.  `data` owns `size` initialized bytes in a
# `cap`-byte allocation.  Construction from a literal is the compiler's
# literal-to-struct bridge (the byte buffer is filled from the literal's
# UTF-8 bytes at the call); every other operation is ordinary library code
# over the byte buffer.  Slicing and the result APIs (`find`/`rfind`/
# `startswith`/`endswith`/`split`) work in byte offsets, like `byte_length`.
#
# `s[codepoint=i]` yields a `Codepoint` value carrying both the decoded
# scalar and the character's text.  `s[grapheme=i]` and `count_graphemes()`
# segment extended grapheme clusters with a documented UAX #29 subset: a
# hand-maintained essentials classifier plus arithmetic Hangul, with GB11
# simplified to "never break after ZWJ" and GB9b (Prepend) omitted.

from std.memory.alloc import unsafe_alloc

from std.collections.list import List
from std.optional import Optional
from std.span import Span

from std.iterable import Iterable, Iterator, StopIteration
from std._string_tables import _lower_table, _pow5_table, _upper_table, _upper2_table, _upper3_table

# Shared strict contiguous-slice bounds checking with the audited head's
# abort messages (upstream std/collections/check_bounds.mojo): start
# out-of-bounds, end out-of-bounds, and reversed bounds each abort with the
# index and the valid range interpolated. Lives here (not in a collections
# module) so the collection modules and String share it without an import
# cycle through the prelude's String binding.
struct _BoundsMessage(Movable, Writer):
    var text: String

    def __init__(out self):
        self.text = String("")

    def write_string(mut self, chunk: String):
        self.text = self.text + chunk


def check_slice_bounds(start: Int, end: Int, length: Int):
    if start < 0 or start > length:
        var message = _BoundsMessage()
        message.write(
            "slice start index ", start, " is out of bounds, valid range is 0 to ", length
        )
        _mojito_abort(message.text)
    if end < 0 or end > length:
        var message = _BoundsMessage()
        message.write(
            "slice end index ", end, " is out of bounds, valid range is 0 to ", length
        )
        _mojito_abort(message.text)
    if start > end:
        var message = _BoundsMessage()
        message.write(
            "slice start index ", start, " is greater than slice end index ", end
        )
        _mojito_abort(message.text)


# Fixed-width hex-record tables (`std._string_tables`, generated from the
# pinned upstream lookups): a table is a string literal of `width`-byte
# records whose first six hex digits are an ascending codepoint key, so a
# lookup is a binary search over the literal's bytes with no compile-time
# array constant behind it.
def _hex_at(table: StringSpan, at: Int, width: Int) -> Int:
    var value = 0
    var i = 0
    while i < width:
        var b = Int(table._data[at + i])
        value = value * 16 + (b - 48 if b <= 57 else b - 87)
        i += 1
    return value


def _hex_u64_at(table: StringSpan, at: Int) -> UInt64:
    var value = UInt64(0)
    var i = 0
    while i < 16:
        var b = Int(table._data[at + i])
        value = value * UInt64(16) + UInt64(b - 48 if b <= 57 else b - 87)
        i += 1
    return value


# The index of the record keyed by `scalar`, or -1.
def _table_find(table: StringSpan, width: Int, scalar: Int) -> Int:
    var low = 0
    var high = table.byte_length() // width
    while low < high:
        var mid = (low + high) // 2
        var key = _hex_at(table, mid * width, 6)
        if key == scalar:
            return mid
        if key < scalar:
            low = mid + 1
        else:
            high = mid
    return -1


# Unicode 16 case mapping (upstream `_unicode.mojo`): the simple lowercase
# table, and the simple uppercase table plus the two- and three-codepoint
# SpecialCasing uppercase tables (`ß` -> `SS`, `ﬃ` -> `FFI`, ...).
def _lower_mapping(lower: StringSpan, scalar: Int) -> Int:
    var index = _table_find(lower, 12, scalar)
    if index < 0:
        return scalar
    return _hex_at(lower, index * 12 + 6, 6)


def _has_lower_mapping(lower: StringSpan, scalar: Int) -> Bool:
    return _table_find(lower, 12, scalar) >= 0


def _has_upper_mapping(
    upper: StringSpan, upper2: StringSpan, upper3: StringSpan, scalar: Int
) -> Bool:
    if _table_find(upper, 12, scalar) >= 0:
        return True
    if _table_find(upper2, 18, scalar) >= 0:
        return True
    return _table_find(upper3, 24, scalar) >= 0


def _append_scalar(mut out: String, scalar: Int):
    var text = Codepoint._encode_utf8(scalar)
    out._append_bytes_of(text, 0, text.size)


# Append the uppercase mapping of `scalar` (the `width` bytes of `src` at
# `at`) to `out`: one to three codepoints, or the original bytes when the
# tables have no entry.
def _append_uppercased(
    mut out: String,
    upper: StringSpan,
    upper2: StringSpan,
    upper3: StringSpan,
    src: StringSpan,
    at: Int,
    width: Int,
    scalar: Int,
):
    var index = _table_find(upper, 12, scalar)
    if index >= 0:
        _append_scalar(out, _hex_at(upper, index * 12 + 6, 6))
        return
    index = _table_find(upper2, 18, scalar)
    if index >= 0:
        _append_scalar(out, _hex_at(upper2, index * 18 + 6, 6))
        _append_scalar(out, _hex_at(upper2, index * 18 + 12, 6))
        return
    index = _table_find(upper3, 24, scalar)
    if index >= 0:
        _append_scalar(out, _hex_at(upper3, index * 24 + 6, 6))
        _append_scalar(out, _hex_at(upper3, index * 24 + 12, 6))
        _append_scalar(out, _hex_at(upper3, index * 24 + 18, 6))
        return
    out._append_bytes_of(src, at, width)


def _too_large_suffix() -> String:
    return " String expresses an integer too large to store in Int."


def _str_to_base_error(base: Int, str: String) -> String:
    return "String is not convertible to integer with base " + String(base) + ": '" + str + "'"


# Integer parsing with Python's literal rules (upstream `atol`): optional
# POSIX-space padding and sign, a `0b`/`0o`/`0x` prefix when it matches the
# base (base 0 detects the base from the prefix), single `_` separators
# between digits, and an overflow check against `Int`.
def atol(str: String, base: Int = 10) raises -> Int:
    if base != 0 and (base < 2 or base > 36):
        raise Error("Base must be >= 2 and <= 36, or 0.")
    var str_len = str.size
    var start = 0
    while start < str_len and str._is_posix_space_byte(Int(str.data[start])):
        start += 1
    if start >= str_len:
        raise Error(_str_to_base_error(base, str))
    var is_negative = False
    var first = Int(str.data[start])
    if first == 43 or first == 45:
        is_negative = first == 45
        start += 1
    if start >= str_len:
        raise Error(_str_to_base_error(base, str))
    var real_base = base
    var has_prefix = False
    if base == 0:
        if start == str_len - 1:
            real_base = 10
        elif Int(str.data[start]) == 48:
            var second = Int(str.data[start + 1])
            if second == 98 or second == 66:
                real_base = 2
                start += 2
                has_prefix = True
            elif second == 111 or second == 79:
                real_base = 8
                start += 2
                has_prefix = True
            elif second == 120 or second == 88:
                real_base = 16
                start += 2
                has_prefix = True
            else:
                # Only "0", "0_0", ... are legal without a prefix.
                var was_underscore = False
                var i = start + 1
                while i < str_len:
                    var b = Int(str.data[i])
                    if b == 95:
                        if was_underscore:
                            raise Error(_str_to_base_error(base, str))
                        was_underscore = True
                    elif b != 48:
                        raise Error(_str_to_base_error(base, str))
                    else:
                        was_underscore = False
                    i += 1
                real_base = 10
        elif Int(str.data[start]) >= 49 and Int(str.data[start]) <= 57:
            real_base = 10
        else:
            raise Error(_str_to_base_error(base, str))
    elif start + 1 < str_len and Int(str.data[start]) == 48:
        var second = Int(str.data[start + 1])
        if (base == 2 and (second == 98 or second == 66)) or (
            base == 8 and (second == 111 or second == 79)
        ) or (base == 16 and (second == 120 or second == 88)):
            start += 2
            has_prefix = True
    var limit = 9223372036854775807
    var pos = start
    # A negative number accumulates negatively so that `Int.MIN` (one more
    # in magnitude than `Int.MAX`) parses without overflow.
    var result = 0
    var found_digit = False
    var trailing = str_len
    var was_underscore = not (has_prefix and (real_base == 2 or real_base == 8 or real_base == 16))
    while pos < str_len:
        var b = Int(str.data[pos])
        if b == 95:
            if was_underscore:
                raise Error(_str_to_base_error(base, str))
            was_underscore = True
            pos += 1
            continue
        was_underscore = False
        var digit = -1
        if b >= 48 and b <= 57:
            digit = b - 48
        elif b >= 97 and b <= 122:
            digit = b - 97 + 10
        elif b >= 65 and b <= 90:
            digit = b - 65 + 10
        elif str._is_posix_space_byte(b):
            trailing = pos
            break
        if digit < 0 or digit >= real_base:
            raise Error(_str_to_base_error(base, str))
        found_digit = True
        var bound = (limit - digit) // real_base
        if is_negative:
            # ceil((limit + 1 - digit) / base) without forming limit + 1.
            if (limit - digit) % real_base == real_base - 1:
                bound += 1
            if result < -bound:
                raise Error(_str_to_base_error(base, str) + _too_large_suffix())
            result = result * real_base - digit
        else:
            if result > bound:
                raise Error(_str_to_base_error(base, str) + _too_large_suffix())
            result = result * real_base + digit
        pos += 1
    if was_underscore or not found_digit:
        raise Error(_str_to_base_error(base, str))
    while trailing < str_len:
        if not str._is_posix_space_byte(Int(str.data[trailing])):
            raise Error(_str_to_base_error(base, str))
        trailing += 1
    return result


def _pow10(exponent: Int) -> Float64:
    var result = 1.0
    var i = 0
    while i < exponent:
        result = result * 10.0
        i += 1
    return result


def _float_inf() -> Float64:
    var big = 1.0e308
    return big * 10.0


def _float_nan() -> Float64:
    var inf = _float_inf()
    return inf - inf


# Floating-point parsing: upstream `atof` (`_parsing_numbers`). The text is
# stripped (POSIX space, a `+` prefix, an `f`/`F` suffix) and signed,
# checked for `nan`/`inf`/`infinity`, then scanned right to left into
# 24-digit significand and exponent buffers (upstream's `CONTAINER_SIZE`,
# so longer numbers raise) that Clinger's fast path or the Eisel-Lemire
# algorithm (arXiv 2101.11408, algorithm 1) converts with correct rounding
# over the generated 128-bit power-of-five table.
def _float_error(str: String) -> String:
    return "String is not convertible to float: '" + str + "'"


def _atof_is_nan(text: StringSpan) -> Bool:
    if text._size != 3:
        return False
    return (
        (Int(text._data[0]) | 32) == 110
        and (Int(text._data[1]) | 32) == 97
        and (Int(text._data[2]) | 32) == 110
    )


# The suffix strip has already taken the `f` of `inf` (upstream's quirk).
def _atof_is_inf(text: StringSpan) -> Bool:
    if text._size == 2:
        return (Int(text._data[0]) | 32) == 105 and (Int(text._data[1]) | 32) == 110
    if text._size != 8:
        return False
    return text.lower() == "infinity"


def _atof_zero_digits() -> List[Int]:
    var digits = List[Int]()
    var i = 0
    while i < 24:
        digits.append(0)
        i += 1
    return digits^


# The value of a 24-digit buffer (upstream `to_integer`); above `UInt64`'s
# range it raises with the digits sans leading zeros.
def _atof_to_integer(digits: List[Int]) raises -> UInt64:
    var limit = UInt64(0) - UInt64(1)
    var value = UInt64(0)
    var i = 0
    while i < 24:
        var digit = UInt64(digits[i])
        if value > (limit - digit) // UInt64(10):
            var text = String()
            var j = 0
            while j < 24 and digits[j] == 0:
                j += 1
            while j < 24:
                text.write(digits[j])
                j += 1
            raise Error("The string is too large to be converted to an integer: '" + text + "'.")
        value = value * UInt64(10) + digit
        i += 1
    return value


# Upstream `_get_w_and_q_from_float_string`: read right to left, filling
# the exponent buffer until a dot or `e` proves the digits belong to the
# significand; the dot's position becomes a negative exponent adjustment.
def _atof_scan(text: StringSpan) raises -> Tuple[UInt64, Int]:
    var size = text._size
    var first = 0 if size == 0 else Int(text._data[0])
    if not ((first >= 48 and first <= 57) or first == 46):
        raise Error(
            "The first character of '"
            + text.to_string()
            + "' should be a digit or dot to convert it to a float."
        )
    var last = Int(text._data[size - 1])
    if not ((last >= 48 and last <= 57) or last == 46):
        raise Error(
            "The last character of '"
            + text.to_string()
            + "' should be a digit or dot to convert it to a float."
        )
    var exponent = _atof_zero_digits()
    var significand = _atof_zero_digits()
    var writing_exponent = True
    var array_index = 24
    var additional_exponent = 0
    var exponent_multiplier = 1
    var dot_or_e_found = False
    var i = size - 1
    while i >= 0:
        array_index -= 1
        if array_index < 0:
            raise Error("The number is too long, it's not supported yet. '" + text.to_string() + "'")
        var b = Int(text._data[i])
        if b == 46:
            dot_or_e_found = True
            if writing_exponent:
                # The digits so far were the significand, not an exponent.
                significand = exponent.copy()
                exponent = _atof_zero_digits()
                writing_exponent = False
            additional_exponent = 24 - array_index - 1
            array_index += 1
        elif b == 45:
            exponent_multiplier = -1
        elif b == 43:
            pass
        elif b == 101 or b == 69:
            dot_or_e_found = True
            writing_exponent = False
            array_index = 24
        elif b >= 48 and b <= 57:
            if writing_exponent:
                exponent[array_index] = b - 48
            else:
                significand[array_index] = b - 48
        else:
            raise Error("Invalid character(s) in the number: '" + text.to_string() + "'")
        i -= 1
    if not dot_or_e_found:
        significand = exponent.copy()
        exponent = _atof_zero_digits()
    var q = exponent_multiplier * Int(_atof_to_integer(exponent)) - additional_exponent
    var w = _atof_to_integer(significand)
    return (w, q)


# Powers of ten and integers below 2**53 are exact, so their product or
# quotient rounds once.
def _atof_clinger(w: UInt64, q: Int) -> Float64:
    if q >= 0:
        return Float64(w) * _pow10(q)
    return Float64(w) / _pow10(-q)


def _count_leading_zeros(value: UInt64) -> Int:
    if value == UInt64(0):
        return 64
    var count = 0
    var v = value
    while (v >> UInt64(63)) == UInt64(0):
        v = v << UInt64(1)
        count += 1
    return count


# The 128-bit product of two 64-bit words as (high, low), via 32-bit limbs.
def _mul_u64_wide(a: UInt64, b: UInt64) -> Tuple[UInt64, UInt64]:
    var mask = UInt64(0xFFFFFFFF)
    var a_lo = a & mask
    var a_hi = a >> UInt64(32)
    var b_lo = b & mask
    var b_hi = b >> UInt64(32)
    var ll = a_lo * b_lo
    var lh = a_lo * b_hi
    var hl = a_hi * b_lo
    var hh = a_hi * b_hi
    var mid = (ll >> UInt64(32)) + (lh & mask) + (hl & mask)
    var low = (ll & mask) | ((mid & mask) << UInt64(32))
    var high = hh + (lh >> UInt64(32)) + (hl >> UInt64(32)) + (mid >> UInt64(32))
    return (high, low)


# Upstream `get_128_bit_truncated_product`: `w` times the 128-bit power of
# five for `q`, refined by the next word when the top 55 bits are all set.
def _atof_truncated_product(w: UInt64, q: Int) -> Tuple[UInt64, UInt64]:
    var table = _pow5_table()
    var index = 2 * (q + 342)
    var product = _mul_u64_wide(w, _hex_u64_at(table, 16 * index))
    var high = product[0]
    var low = product[1]
    var precision_mask = (UInt64(1) << UInt64(55)) - UInt64(1)
    if (high & precision_mask) == precision_mask:
        var second = _mul_u64_wide(w, _hex_u64_at(table, 16 * (index + 1)))
        low = low + second[0]
        if second[0] > low:
            high = high + UInt64(1)
    return (high, low)


# `m * 2**exponent` exactly: the result is representable by construction,
# so every power-of-two step stays exact.
def _ldexp(m: UInt64, exponent: Int) -> Float64:
    var value = Float64(m)
    var two64 = 1.0
    var i = 0
    while i < 64:
        two64 = two64 * 2.0
        i += 1
    var e = exponent
    while e >= 64:
        value = value * two64
        e -= 64
    while e <= -64:
        value = value / two64
        e += 64
    var rest = 1.0
    var steps = e if e >= 0 else -e
    i = 0
    while i < steps:
        rest = rest * 2.0
        i += 1
    if e >= 0:
        return value * rest
    return value / rest


def _atof_lemire(significand: UInt64, q: Int) -> Float64:
    var w = significand
    if w == UInt64(0):
        return 0.0
    if q < -342:
        return 0.0
    if q > 308:
        return _float_inf()
    var l = _count_leading_zeros(w)
    w = w << UInt64(l)
    var product = _atof_truncated_product(w, q)
    var high = product[0]
    var low = product[1]
    var upper_bit = Int(high >> UInt64(63))
    var m = high >> UInt64(upper_bit + 9)
    var p = (((152170 + 65536) * q) >> 16) + 63 - l + upper_bit
    if p <= (-1022 - 64):
        return 0.0
    if p < -1022:
        # Subnormal: shift the mantissa down, rounding half up.
        var shift = -1022 - p
        m = m >> UInt64(shift)
        if (m & UInt64(1)) == UInt64(1):
            m += UInt64(1)
        m = m >> UInt64(1)
        return _ldexp(m, -1074)
    # Round ties to even where the truncated product could be exactly half.
    if q >= -4 and q <= 23:
        if low <= UInt64(1):
            if (m & UInt64(3)) == UInt64(1):
                var ratio = high // m
                if ratio != UInt64(0):
                    if (ratio & (ratio - UInt64(1))) == UInt64(0):
                        m -= UInt64(2)
    if (m & UInt64(1)) == UInt64(1):
        m += UInt64(1)
    m = m >> UInt64(1)
    if m == (UInt64(1) << UInt64(53)):
        m = m >> UInt64(1)
        p += 1
    if p > 1023:
        return _float_inf()
    return _ldexp(m, p - 52)


def atof(str: String) raises -> Float64:
    if str.size == 0 or (str.size == 1 and Int(str.data[0]) == 46):
        raise Error(_float_error(str))
    var whole = StringSpan(str)
    var trimmed = whole.strip()
    var unplussed = trimmed.removeprefix("+")
    var unsuffixed = unplussed.removesuffix("f")
    var text = unsuffixed.removesuffix("F")
    var sign = 1.0
    if text.startswith("-"):
        sign = -1.0
        text = text._sub_view(1, text._size)
    if _atof_is_nan(text):
        return _float_nan()
    if _atof_is_inf(text):
        return _float_inf() * sign
    var w = UInt64(0)
    var q = 0
    try:
        var parts = _atof_scan(text)
        w = parts[0]
        q = parts[1]
    except e:
        var message = String()
        message.write(_float_error(str), ". ", e)
        raise Error(message)
    if q >= -22 and q <= 22:
        if w <= (UInt64(1) << UInt64(53)):
            return _atof_clinger(w, q) * sign
    return _atof_lemire(w, q) * sign


# `std.os.PathLike` (re-exported there): a value that names a filesystem
# path. Declared here because the trait names `String` and this module loads
# before `std.os`.
# UTF-8 well-formedness (the Unicode table of valid byte sequences), for the
# validating `String(from_utf8=...)` constructor.
def _is_valid_utf8(bytes: Span[UInt8, _]) -> Bool:
    var i = 0
    var n = len(bytes)
    while i < n:
        var lead = Int(bytes[i])
        if lead < 0x80:
            i += 1
            continue
        var width = 0
        var min_second = 0x80
        var max_second = 0xBF
        if lead >= 0xC2 and lead <= 0xDF:
            width = 2
        elif lead == 0xE0:
            width = 3
            min_second = 0xA0
        elif (lead >= 0xE1 and lead <= 0xEC) or lead == 0xEE or lead == 0xEF:
            width = 3
        elif lead == 0xED:
            width = 3
            max_second = 0x9F
        elif lead == 0xF0:
            width = 4
            min_second = 0x90
        elif lead >= 0xF1 and lead <= 0xF3:
            width = 4
        elif lead == 0xF4:
            width = 4
            max_second = 0x8F
        else:
            return False
        if i + width > n:
            return False
        var second = Int(bytes[i + 1])
        if second < min_second or second > max_second:
            return False
        var k = 2
        while k < width:
            var continuation = Int(bytes[i + k])
            if continuation < 0x80 or continuation > 0xBF:
                return False
            k += 1
        i += width
    return True


trait PathLike:
    def __fspath__(self) -> String:
        ...


struct String(
    Boolable,
    Comparable,
    Copyable,
    Equatable,
    Hashable,
    ImplicitlyCopyable,
    Iterable,
    Movable,
    PathLike,
    Writable,
    Writer,
):
    # `Element` stays an unbound alias: `String` has no origin parameter to
    # forward, and an alias declaration is a legal unbound position upstream
    # (only storage annotations demand bound origin slots).
    comptime Element = StringSpan
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _GraphemeIter[iterable_origin]

    var data: UnsafePointer[Byte]
    var size: Int
    var cap: Int

    @implicit
    def __init__(out self, literal: StringLiteral):
        # The compiler replaces this call: `data`/`size`/`cap` are filled
        # from the literal's UTF-8 bytes.  The body only establishes the
        # field contract and never executes.  `@implicit` lets a literal
        # convert wherever the nominal String is expected.
        self.size = 0
        self.cap = 1
        self.data = unsafe_alloc[Byte](self.cap)

    def __init__(out self):
        self.size = 0
        self.cap = 1
        self.data = unsafe_alloc[Byte](self.cap)

    # 2026-08 stabilization: pre-sized construction. Capacity is a real byte
    # buffer here (the VM's literal/copy bridges manage their own storage).
    def __init__(out self, *, capacity_bytes: Int):
        self.size = 0
        self.cap = capacity_bytes if capacity_bytes > 0 else 1
        self.data = unsafe_alloc[Byte](self.cap)

    # Length without initialization: the caller writes every byte in
    # `[0, length)` through `unsafe_ptr_mut()` before the text is read.
    def __init__(out self, *, unsafe_uninit_length: Int):
        self.size = unsafe_uninit_length
        self.cap = unsafe_uninit_length if unsafe_uninit_length > 0 else 1
        self.data = unsafe_alloc[Byte](self.cap)

    # Upstream's constructor from a NUL-terminated UTF-8 pointer (a C string
    # a libc call returned): the bytes up to the terminator are copied.
    def __init__(out self, *, unsafe_from_utf8_ptr: Pointer[UInt8, _]):
        self.size = Int(external_call["strlen", UInt](unsafe_from_utf8_ptr))
        self.cap = self.size if self.size > 0 else 1
        self.data = unsafe_alloc[Byte](self.cap)
        # The source is usually C-owned memory (a `getenv`/`readdir`/`strerror`
        # result), so the bytes are copied by libc rather than read through
        # the provenance-guarded pointer loads.
        var copied = external_call["memcpy", Pointer[UInt8, MutUntrackedOrigin]](
            self.data, unsafe_from_utf8_ptr, UInt(self.size)
        )

    # Upstream's validating constructor from UTF-8 bytes (a file read).
    def __init__(out self, *, from_utf8: Span[UInt8, _]) raises:
        if not _is_valid_utf8(from_utf8):
            raise Error("Cannot construct a String from invalid UTF-8 data")
        self.size = len(from_utf8)
        self.cap = self.size if self.size > 0 else 1
        self.data = unsafe_alloc[Byte](self.cap)
        var i = 0
        while i < self.size:
            self.data[i] = from_utf8[i]
            i += 1

    def __init__(out self, *, copy: Self):
        self.size = copy.size
        self.cap = copy.cap
        self.data = unsafe_alloc[Byte](self.cap)
        var i = 0
        while i < copy.size:
            self.data[i] = copy.data[i]
            i += 1

    def copy(self) -> Self:
        return String(copy: self)

    def __init__(out self, *, deinit move: Self):
        self.size = move.size
        self.cap = move.cap
        self.data = move.data^

    def __deinit__(deinit self):
        self.data.unsafe_free()

    # 2026-08 stabilization: reserve at least the requested capacity; a
    # current capacity at or above it is a no-op.
    def reserve_bytes(mut self, new_capacity_bytes: Int, /):
        if new_capacity_bytes <= self.cap:
            return
        var new_data = unsafe_alloc[Byte](new_capacity_bytes)
        var i = 0
        while i < self.size:
            # Move rather than read: a never-written byte (an
            # `unsafe_uninit_length` slot) forwards its state instead of
            # trapping on the VM.
            new_data[i] = self.data.unsafe_offset(i).unsafe_take_pointee()
            i += 1
        self.data.unsafe_free()
        self.data = new_data
        self.cap = new_capacity_bytes

    # No `__len__`: a UTF-8 length is ambiguous (upstream `@unavailable`);
    # spell the unit — `byte_length()`, `len(s.codepoints())`, or
    # `len(s.graphemes())`.
    def byte_length(self) -> Int:
        return self.size

    def __eq__(self, other: Self) -> Bool:
        if self.size != other.size:
            return False
        var i = 0
        while i < self.size:
            if Int(self.data[i]) != Int(other.data[i]):
                return False
            i += 1
        return True

    def __ne__(self, other: Self) -> Bool:
        return not (self == other)

    # Upstream compares an owned String against a view bytewise; the operator
    # selects this overload by the right operand's type.
    def __eq__(self, other: StringSpan) -> Bool:
        if self.size != other._size:
            return False
        var i = 0
        while i < self.size:
            if Int(self.data[i]) != Int(other._data[i]):
                return False
            i += 1
        return True

    def __ne__(self, other: StringSpan) -> Bool:
        return not (self == other)

    def __lt__(self, other: Self) -> Bool:
        var shared = self.size
        if other.size < shared:
            shared = other.size
        var i = 0
        while i < shared:
            if Int(self.data[i]) < Int(other.data[i]):
                return True
            if Int(other.data[i]) < Int(self.data[i]):
                return False
            i += 1
        return self.size < other.size

    def __le__(self, other: Self) -> Bool:
        return not (other < self)

    def __gt__(self, other: Self) -> Bool:
        return other < self

    def __ge__(self, other: Self) -> Bool:
        return not (self < other)

    def __add__(self, other: Self) -> Self:
        var result = String("")
        result.data.unsafe_free()
        result.data = unsafe_alloc[Byte](self.size + other.size)
        result.size = self.size + other.size
        result.cap = result.size
        var i = 0
        while i < self.size:
            result.data[i] = self.data[i]
            i += 1
        var j = 0
        while j < other.size:
            result.data[self.size + j] = other.data[j]
            j += 1
        return result^

    def __iadd__(mut self, other: Self):
        var data = unsafe_alloc[Byte](self.size + other.size)
        var i = 0
        while i < self.size:
            data[i] = self.data[i]
            i += 1
        var j = 0
        while j < other.size:
            data[self.size + j] = other.data[j]
            j += 1
        self.data.unsafe_free()
        self.data = data
        self.size = self.size + other.size
        self.cap = self.size

    def __bool__(self) -> Bool:
        return self.size > 0

    # Concatenates the string `n` times; a non-positive count is empty.
    def __mul__(self, n: Int) -> String:
        var result = String()
        var i = 0
        while i < n:
            result._append_bytes_of(self, 0, self.size)
            i += 1
        return result^

    # `Writer` conformance: `s.write(a, b, ...)` appends each argument's
    # written text to this buffer (amortized doubling growth).
    def write_string(mut self, chunk: String):
        self._append_bytes_of(chunk, 0, chunk.size)

    # The result APIs (search, affix tests, replace, split, case, predicates,
    # justification, and the strip family) live on `StringSpan` in upstream's
    # shape; every String spelling forwards through a view of this buffer.
    # Needle/affix/separator arguments are `StringSpan`s: a String or a
    # literal converts implicitly.
    def __contains__(self, sub: StringSpan) -> Bool:
        return StringSpan(self).__contains__(sub)

    def find(self, substr: StringSpan, start: Int = 0) -> Int:
        return StringSpan(self).find(substr, start)

    def rfind(self, substr: StringSpan, start: Int = 0) -> Int:
        return StringSpan(self).rfind(substr, start)

    def count(self, substr: StringSpan) -> Int:
        return StringSpan(self).count(substr)

    def startswith(self, prefix: StringSpan, start: Int = 0, end: Int = -1) -> Bool:
        return StringSpan(self).startswith(prefix, start, end)

    def endswith(self, suffix: StringSpan, start: Int = 0, end: Int = -1) -> Bool:
        return StringSpan(self).endswith(suffix, start, end)

    def replace(self, old: StringSpan, new: StringSpan) -> String:
        return StringSpan(self).replace(old, new)

    # Joins the written text of `elems` with this string between them; a
    # `List[T]` argument converts through Span's implicit constructor.
    def join[T: Copyable & Writable](self, elems: Span[T, _]) -> String:
        var result = String()
        var i = 0
        while i < len(elems):
            if i > 0:
                result._append_bytes_of(self, 0, self.size)
            result.write(elems[i])
            i += 1
        return result^

    # Eager owned pieces rather than current Mojo's borrowed StringSpan
    # views (the recorded eager-result divergence); upstream's four overloads.
    def split(self, sep: StringSpan) -> List[String]:
        return StringSpan(self).split(sep)

    def split(self, sep: StringSpan, maxsplit: Int) -> List[String]:
        return StringSpan(self).split(sep, maxsplit)

    def split(self, sep: NoneType = None) -> List[String]:
        return StringSpan(self).split()

    def split(self, sep: NoneType = None, *, maxsplit: Int) -> List[String]:
        return StringSpan(self).split(maxsplit=maxsplit)

    def splitlines(self, keepends: Bool = False) -> List[String]:
        return StringSpan(self).splitlines(keepends)

    def upper(self) -> String:
        return StringSpan(self).upper()

    def lower(self) -> String:
        return StringSpan(self).lower()

    def isupper(self) -> Bool:
        return StringSpan(self).isupper()

    def islower(self) -> Bool:
        return StringSpan(self).islower()

    def isspace(self) -> Bool:
        return StringSpan(self).isspace()

    def is_ascii_digit(self) -> Bool:
        return StringSpan(self).is_ascii_digit()

    def is_ascii_printable(self) -> Bool:
        return StringSpan(self).is_ascii_printable()

    def ascii_rjust(self, width: Int, fillchar: StringSpan = " ") -> String:
        return StringSpan(self).ascii_rjust(width, fillchar)

    def ascii_ljust(self, width: Int, fillchar: StringSpan = " ") -> String:
        return StringSpan(self).ascii_ljust(width, fillchar)

    def ascii_center(self, width: Int, fillchar: StringSpan = " ") -> String:
        return StringSpan(self).ascii_center(width, fillchar)

    # Byte access: a borrowed `Span[Byte]` over the buffer and the raw
    # interior pointer (current Mojo's `as_bytes`/`unsafe_ptr`).
    def as_bytes(ref self) -> Span[Byte, origin_of(self)]:
        return Span[Byte, origin_of(self)](unsafe_ptr=self.unsafe_ptr(), length=self.size)

    def unsafe_ptr(ref self) -> Pointer[Byte, origin_of(self)._get_owned_interior["bytes"]]:
        return self.data.unsafe_origin_cast[origin_of(self)._get_owned_interior["bytes"]]()

    # Upstream's mutable byte pointer: reserves `capacity` bytes first (0 is
    # no growth) so the caller may write through the returned pointer.
    def unsafe_ptr_mut(
        mut self, capacity: Int = 0
    ) -> Pointer[Byte, origin_of(self)._get_owned_interior["bytes"]]:
        self.reserve_bytes(capacity)
        return self.data.unsafe_origin_cast[origin_of(self)._get_owned_interior["bytes"]]()

    def capacity_bytes(self) -> Int:
        return self.cap

    # Upstream's C-string view: writes a NUL terminator past the text (the
    # byte is not part of the string and every mutation may overwrite it, so
    # the view is taken fresh at each call site) and views the buffer as
    # `c_char` bytes.
    def as_c_string_slice(mut self) -> CStringSlice[origin_of(self)]:
        self.reserve_bytes(self.size + 1)
        self.data[self.size] = 0
        return CStringSlice(self)

    # `os.PathLike`: a String is its own filesystem path.
    def __fspath__(self) -> String:
        return self

    # Grow with `fill_byte` (ASCII only) or shrink to a codepoint boundary;
    # violations abort with upstream's assertion texts.
    def resize(mut self, length: Int, fill_byte: UInt8 = 0):
        if Int(fill_byte) >= 128:
            _mojito_abort("Fill byte is the start of a multi-byte character.")
        if length > self.size:
            self.reserve_bytes(length)
            var i = self.size
            while i < length:
                self.data[i] = fill_byte
                i += 1
        elif not self._is_codepoint_boundary(length):
            var message = String()
            message.write(
                "String shrunk to length ", length, " which does not lie on a codepoint boundary."
            )
            _mojito_abort(message)
        self.size = length

    # Grow leaving the new bytes uninitialized (written through
    # `unsafe_ptr_mut()`), or shrink to a codepoint boundary.
    def resize(mut self, *, unsafe_uninit_length: Int):
        if unsafe_uninit_length > self.size:
            self.reserve_bytes(unsafe_uninit_length)
        elif not self._is_codepoint_boundary(unsafe_uninit_length):
            var message = String()
            message.write(
                "String shrunk to length ",
                unsafe_uninit_length,
                " which does not lie on a codepoint boundary.",
            )
            _mojito_abort(message)
        self.size = unsafe_uninit_length

    def append(mut self, codepoint: Codepoint):
        self._append_bytes_of(codepoint._text, 0, codepoint._text.size)

    # Numeric parsing through the free `atol`/`atof` (`Int(s)` / `Float64(s)`).
    def __int__(self) raises -> Int:
        return atol(self)

    def __float__(self) raises -> Float64:
        return atof(self)

    # Codepoint-level iteration: decoded `Codepoint` values, borrowed
    # single-codepoint sub-views, and the grapheme-cluster views ordinary
    # iteration yields.
    def codepoints(self) -> _CodepointIter[origin_of(self)]:
        return _CodepointIter(StringSpan(self), 0)

    def codepoint_slices(self) -> _CodepointSliceIter[origin_of(self)]:
        return _CodepointSliceIter(StringSpan(self), 0)

    def graphemes(self) -> _GraphemeIter[origin_of(self)]:
        return _GraphemeIter(StringSpan(self), 0)

    # The strip family returns borrowed views of this buffer (the view's
    # result carries this String's origin).
    def strip(self) -> StringSpan[origin_of(self)]:
        return StringSpan(self).strip()

    def strip(self, chars: StringSpan) -> StringSpan[origin_of(self)]:
        return StringSpan(self).strip(chars)

    def lstrip(self) -> StringSpan[origin_of(self)]:
        return StringSpan(self).lstrip()

    def lstrip(self, chars: StringSpan) -> StringSpan[origin_of(self)]:
        return StringSpan(self).lstrip(chars)

    def rstrip(self) -> StringSpan[origin_of(self)]:
        return StringSpan(self).rstrip()

    def rstrip(self, chars: StringSpan) -> StringSpan[origin_of(self)]:
        return StringSpan(self).rstrip(chars)

    def removeprefix(self, prefix: StringSpan, /) -> StringSpan[origin_of(self)]:
        return StringSpan(self).removeprefix(prefix)

    def removesuffix(self, suffix: StringSpan, /) -> StringSpan[origin_of(self)]:
        return StringSpan(self).removesuffix(suffix)

    # Append `count` bytes of `src` from byte offset `start`, doubling the
    # capacity when the buffer is full. The result builders on `StringSpan`
    # accumulate into a String through this.
    def _append_bytes_of(mut self, src: StringSpan, start: Int, count: Int):
        var needed = self.size + count
        if needed > self.cap:
            var new_cap = self.cap * 2
            if new_cap < needed:
                new_cap = needed
            self.reserve_bytes(new_cap)
        var i = 0
        while i < count:
            self.data[self.size + i] = src._data[start + i]
            i += 1
        self.size = needed

    # A borrowed view over bytes `[start, end)` of this buffer.
    def _byte_view(self, start: Int, end: Int) -> StringSpan[origin_of(self)]:
        var view = StringSpan(self)
        view._data = view._data.unsafe_offset(start)
        view._size = end - start
        return view^

    # Byte helpers the iterators and the numeric parsers still call on an
    # owned String; the view owns the algorithms.
    def _scalar_at(self, at: Int, width: Int) -> Int:
        return StringSpan(self)._scalar_at(at, width)

    def _lead_width(self, lead: Int) -> Int:
        return StringSpan(self)._lead_width(lead)

    def _is_posix_space_byte(self, b: Int) -> Bool:
        return StringSpan(self)._is_posix_space_byte(b)

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher.update(StringSpan(self))

    def __getitem__(self, *, byte: Int) raises -> Byte:
        if byte < 0:
            raise Error("String byte index out of range")
        if byte >= self.size:
            raise Error("String byte index out of range")
        return self.data[byte]

    # Codepoint and grapheme indexing and counting live on the view;
    # counting never raises (upstream), indexing raises on a bad index.
    def __getitem__(self, *, codepoint: Int) raises -> Codepoint:
        var view = StringSpan(self)
        return view[codepoint=codepoint]

    def count_codepoints(self) -> Int:
        return StringSpan(self).count_codepoints()

    def __getitem__(self, *, grapheme: Int) raises -> Self:
        var view = StringSpan(self)
        return view[grapheme=grapheme]

    def count_graphemes(self) -> Int:
        return StringSpan(self).count_graphemes()

    # Ordinary String iteration yields borrowed grapheme-cluster StringSpan
    # views (current Mojo); `reversed(s)` walks the clusters back to front.
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return _GraphemeIter(StringSpan(self), 0)

    def __reversed__(self) -> _GraphemeReversedIter[origin_of(self)]:
        return _GraphemeReversedIter(StringSpan(self), self.size, 0, False)

    def codepoint_slices_reversed(self) -> _CodepointSliceReversedIter[origin_of(self)]:
        return _CodepointSliceReversedIter(StringSpan(self), self.size)

    def graphemes_reversed(self) -> _GraphemeReversedIter[origin_of(self)]:
        return _GraphemeReversedIter(StringSpan(self), self.size, 0, False)

    def bytes(self) -> _BytesIter[origin_of(self)]:
        return _BytesIter(StringSpan(self), 0)

    # The views before and after the `n`-th grapheme-cluster boundary.
    def split_at_grapheme(
        self, n: Int
    ) -> Tuple[StringSpan[origin_of(self)], StringSpan[origin_of(self)]]:
        return StringSpan(self).split_at_grapheme(n)

    # Strict keyword slices (current Mojo bounds): positional String slicing
    # was removed upstream, so byte and codepoint ranges are spelled
    # explicitly and violations abort. Byte endpoints must fall on UTF-8
    # codepoint boundaries; the result is a borrowed `StringSpan` view of
    # this String's buffer.
    def __getitem__(ref self, *, byte: ContiguousSlice) -> StringSpan[origin_of(self)]:
        var start = byte.start.or_else(0)
        var end = byte.end.or_else(self.size)
        check_slice_bounds(start, end, self.size)
        if not self._is_codepoint_boundary(start):
            _mojito_abort("String byte slice endpoint is not a codepoint boundary")
        if not self._is_codepoint_boundary(end):
            _mojito_abort("String byte slice endpoint is not a codepoint boundary")
        var view = StringSpan(self)
        view._data = view._data.unsafe_offset(start)
        view._size = end - start
        return view^

    def __getitem__(ref self, *, codepoint: ContiguousSlice) -> StringSpan[origin_of(self)]:
        var view = StringSpan(self)
        var total = view.count_codepoints()
        var start = codepoint.start.or_else(0)
        var end = codepoint.end.or_else(total)
        check_slice_bounds(start, end, total)
        var start_byte = view._codepoint_offset(start)
        var end_byte = view._codepoint_offset(end)
        view._data = view._data.unsafe_offset(start_byte)
        view._size = end_byte - start_byte
        return view^

    # Whether `offset` falls between UTF-8 sequences (or at either buffer
    # end): a continuation byte marks an interior position.
    def _is_codepoint_boundary(self, offset: Int) -> Bool:
        if offset == 0 or offset == self.size:
            return True
        var b = Int(self.data[offset])
        if b < 128:
            return True
        return b >= 192

    def _with_bytes(self, start: Int, count: Int) -> Self:
        var result = String("")
        result.data.unsafe_free()
        result.data = unsafe_alloc[Byte](count)
        result.size = count
        result.cap = count
        var i = 0
        while i < count:
            result.data[i] = self.data[start + i]
            i += 1
        return result^

    def _as_string_literal(self) -> StringLiteral:
        # The compiler replaces this call: the byte buffer reads back as a
        # compile-time string value (the struct-to-literal bridge).  The body
        # only establishes the signature and never executes.
        return ""

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self._as_string_literal())

    # Upstream's single-quoted repr with backslash escapes for `\\`, `'`,
    # newline, tab, and carriage return.
    def write_repr_to(self, mut writer: Some[Writer]):
        var out = String("'")
        var run_start = 0
        var i = 0
        while i < self.size:
            var b = Int(self.data[i])
            if b == 92 or b == 39 or b == 10 or b == 9 or b == 13:
                out._append_bytes_of(self, run_start, i - run_start)
                if b == 92:
                    out._append_bytes_of("\\\\", 0, 2)
                elif b == 39:
                    out._append_bytes_of("\\'", 0, 2)
                elif b == 10:
                    out._append_bytes_of("\\n", 0, 2)
                elif b == 9:
                    out._append_bytes_of("\\t", 0, 2)
                else:
                    out._append_bytes_of("\\r", 0, 2)
                run_start = i + 1
            i += 1
        out._append_bytes_of(self, run_start, self.size - run_start)
        out._append_bytes_of("'", 0, 1)
        writer.write(out)

# A decoded Unicode scalar together with its character text.  Produced by
# `String.__getitem__(*, codepoint=...)`, which transfers the owned bytes, or by the public
# `Codepoint.from_u32(scalar)` (Mojito is Int-based), which UTF-8-encodes
# the scalar in ordinary library code through runtime `Byte(Int)`
# conversions.
struct Codepoint(
    Comparable, Copyable, Equatable, Deinitable, ImplicitlyCopyable, Intable, Movable, Writable
):
    var _scalar: Int
    var _text: String

    def __init__(out self, scalar: Int, *, var text: String):
        self._scalar = scalar
        self._text = text^

    # The public scalar constructor: absent for negatives, the surrogate
    # range, and values beyond U+10FFFF.
    @staticmethod
    def from_u32(scalar: Int) -> Optional[Codepoint]:
        if scalar < 0:
            return Optional[Codepoint]()
        if scalar >= 0xD800 and scalar <= 0xDFFF:
            return Optional[Codepoint]()
        if scalar > 0x10FFFF:
            return Optional[Codepoint]()
        var text = Codepoint._encode_utf8(scalar)
        return Optional[Codepoint](Codepoint(scalar, text: text^))

    # UTF-8-encode a valid Unicode scalar into a fresh String byte buffer:
    # the lead byte carries the sequence width, continuations carry six bits
    # each.
    @staticmethod
    def _encode_utf8(scalar: Int) -> String:
        var result = String("")
        result.data.unsafe_free()
        if scalar < 0x80:
            result.data = unsafe_alloc[Byte](1)
            result.size = 1
            result.cap = 1
            result.data[0] = Byte(scalar)
        elif scalar < 0x800:
            result.data = unsafe_alloc[Byte](2)
            result.size = 2
            result.cap = 2
            result.data[0] = Byte(192 + scalar // 64)
            result.data[1] = Byte(128 + scalar % 64)
        elif scalar < 0x10000:
            result.data = unsafe_alloc[Byte](3)
            result.size = 3
            result.cap = 3
            result.data[0] = Byte(224 + scalar // 4096)
            result.data[1] = Byte(128 + (scalar // 64) % 64)
            result.data[2] = Byte(128 + scalar % 64)
        else:
            result.data = unsafe_alloc[Byte](4)
            result.size = 4
            result.cap = 4
            result.data[0] = Byte(240 + scalar // 262144)
            result.data[1] = Byte(128 + (scalar // 4096) % 64)
            result.data[2] = Byte(128 + (scalar // 64) % 64)
            result.data[3] = Byte(128 + scalar % 64)
        return result^

    def __int__(self) -> Int:
        return self._scalar

    def is_ascii(self) -> Bool:
        return self._scalar < 128

    def is_ascii_digit(self) -> Bool:
        return self._scalar >= 48 and self._scalar <= 57

    # The default "C" locale: `A`-`Z` / `a`-`z` only.
    def is_ascii_upper(self) -> Bool:
        return self._scalar >= 65 and self._scalar <= 90

    def is_ascii_lower(self) -> Bool:
        return self._scalar >= 97 and self._scalar <= 122

    def is_ascii_printable(self) -> Bool:
        return self._scalar >= 32 and self._scalar <= 126

    # POSIX space: `" \t\n\v\f\r\x1c\x1d\x1e"`.
    def is_posix_space(self) -> Bool:
        var c = self._scalar
        return c == 32 or (c >= 9 and c <= 13) or (c >= 28 and c <= 30)

    # Python's universal separators: POSIX space plus U+0085, U+2028, U+2029.
    def is_python_space(self) -> Bool:
        return self.is_posix_space() or self._scalar == 0x85 or self._scalar == 0x2028 or self._scalar == 0x2029

    def utf8_byte_length(self) -> Int:
        if self._scalar < 0x80:
            return 1
        if self._scalar < 0x800:
            return 2
        if self._scalar < 0x10000:
            return 3
        return 4

    def __eq__(self, other: Self) -> Bool:
        return self._scalar == other._scalar

    def __ne__(self, other: Self) -> Bool:
        return self._scalar != other._scalar

    def __lt__(self, other: Self) -> Bool:
        return self._scalar < other._scalar

    def __le__(self, other: Self) -> Bool:
        return self._scalar <= other._scalar

    def __gt__(self, other: Self) -> Bool:
        return self._scalar > other._scalar

    def __ge__(self, other: Self) -> Bool:
        return self._scalar >= other._scalar

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self._text)


# A borrowed byte view over a String's UTF-8 buffer: current Mojo's
# `StringSpan` (upstream also accepts the older `StringSlice` spelling;
# Mojito emits `StringSpan`). Constructing it from a String lends the
# String's place, so the source stays alive while any view lives and
# mutation conflicts. Keyword indexing mirrors String's vocabulary, and the
# strict keyword slices — including the grapheme slice String itself does
# not offer — return sub-views of the same buffer. The result APIs (search,
# affix tests, replace, split, case, predicates, justification, strip) live
# here and `String` forwards to them, as upstream, including the codepoint
# and grapheme scans (indexing, counting, slicing, forward and reverse
# iteration) that run in place over this buffer.
struct StringSpan[mut: Bool, //, origin: Origin[mut=mut]](
    Boolable,
    Equatable,
    Hashable,
    ImplicitlyCopyable,
    Iterable,
    Movable,
    PathLike,
    Writable,
):
    comptime Element = StringSpan[Self.origin]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _GraphemeIter[iterable_origin]

    var _data: Pointer[Byte, Self.origin._get_owned_interior["bytes"]]
    var _size: Int

    @implicit
    def __init__(out self, ref [Self.origin] src: String):
        self._data = src.data.unsafe_origin_cast[
            origin._get_owned_interior["bytes"]
        ]()
        self._size = src.size

    # Upstream's `StaticString` initializer (the origin is erased here). The
    # compiler replaces this call: `_data`/`_size` view the literal's UTF-8
    # bytes, which live for the whole program. The body only establishes the
    # field contract and never executes. `@implicit` lets a literal convert
    # wherever a view is expected.
    @implicit
    def __init__(out self, literal: StringLiteral):
        self._data = unsafe_alloc[Byte](1).unsafe_origin_cast[
            origin._get_owned_interior["bytes"]
        ]()
        self._size = 0

    # Ordinary StringSpan iteration also yields grapheme-cluster sub-views.
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return _GraphemeIter(self, 0)

    def byte_length(self) -> Int:
        return self._size

    # `os.PathLike`: the viewed text names the path.
    def __fspath__(self) -> String:
        return String(self)

    def __bool__(self) -> Bool:
        return self._size > 0

    # Equality is bytewise against another view or an owned String (upstream's
    # `__eq__` overloads); the operator selects by the right operand's type,
    # and a literal converts to `String`.
    def __eq__(self, rhs: Self) -> Bool:
        if self._size != rhs._size:
            return False
        var i = 0
        while i < self._size:
            if Int(self._data[i]) != Int(rhs._data[i]):
                return False
            i += 1
        return True

    def __eq__(self, rhs: String) -> Bool:
        if self._size != rhs.size:
            return False
        var i = 0
        while i < self._size:
            if Int(self._data[i]) != Int(rhs.data[i]):
                return False
            i += 1
        return True

    # Upstream declares `__ne__` for views only (no `String` overload).
    def __ne__(self, rhs: Self) -> Bool:
        return not (self == rhs)

    def __contains__(self, substr: StringSpan) -> Bool:
        return self._find_from(substr, 0) >= 0

    # Result APIs use byte offsets, like `len` and the byte-wise slice
    # (upstream `string_span.mojo`).  An empty needle matches everywhere
    # (Python semantics): `find` reports 0, `rfind` the byte length, the
    # affix tests True.  A negative `start` counts from the end and clamps.

    def find(self, substr: StringSpan, start: Int = 0) -> Int:
        if substr._size == 0:
            return 0
        if self._size < substr._size + start:
            return -1
        return self._find_from(substr, self._search_start(start))

    def rfind(self, substr: StringSpan, start: Int = 0) -> Int:
        if substr._size == 0:
            return self._size
        if self._size < substr._size + start:
            return -1
        var start_byte = self._search_start(start)
        var at = self._size - substr._size
        while at >= start_byte:
            if self._matches_at(substr, at):
                return at
            at -= 1
        return -1

    def count(self, substr: StringSpan) -> Int:
        if substr._size == 0:
            return self._size + 1
        var total = 0
        var at = self._find_from(substr, 0)
        while at >= 0:
            total += 1
            at = self._find_from(substr, at + substr._size)
        return total

    # `start`/`end` are byte offsets; `end == -1` means the whole string.
    def startswith(self, prefix: StringSpan, start: Int = 0, end: Int = -1) -> Bool:
        if end == -1:
            return self.find(prefix, start) == start
        if start < 0 or end > self._size or end - start < prefix._size:
            return False
        return self._matches_at(prefix, start)

    def endswith(self, suffix: StringSpan, start: Int = 0, end: Int = -1) -> Bool:
        if suffix._size > self._size:
            return False
        if end == -1:
            return self.rfind(suffix, start) + suffix._size == self._size
        if start < 0 or end > self._size or end - start < suffix._size:
            return False
        return self._matches_at(suffix, end - suffix._size)

    # Every occurrence of `old` replaced by `new`; an empty `old` interleaves
    # `new` before every codepoint.
    def replace(self, old: StringSpan, new: StringSpan) -> String:
        var result = String()
        if old._size == 0:
            var at = 0
            while at < self._size:
                var width = self._lead_width(Int(self._data[at]))
                result._append_bytes_of(new, 0, new._size)
                result._append_bytes_of(self, at, width)
                at += width
            return result^
        var start = 0
        var at = self._find_from(old, 0)
        while at >= 0:
            result._append_bytes_of(self, start, at - start)
            result._append_bytes_of(new, 0, new._size)
            start = at + old._size
            at = self._find_from(old, start)
        result._append_bytes_of(self, start, self._size - start)
        return result^

    # Eager owned pieces rather than current Mojo's borrowed views (the
    # recorded eager-result divergence), in upstream's four overloads.  An
    # empty separator yields an empty piece, every codepoint, and an empty
    # piece (upstream ignores `maxsplit` there); `maxsplit` bounds the number
    # of splits.
    def split(self, sep: StringSpan) -> List[String]:
        return self._split_on(sep, -1)

    def split(self, sep: StringSpan, maxsplit: Int) -> List[String]:
        return self._split_on(sep, maxsplit)

    # Whitespace split: runs of Python-space codepoints (POSIX space plus
    # U+0085, U+2028, U+2029) separate pieces and never yield empty ones.
    def split(self, sep: NoneType = None) -> List[String]:
        return self._split_whitespace(-1)

    def split(self, sep: NoneType = None, *, maxsplit: Int) -> List[String]:
        return self._split_whitespace(maxsplit)

    # Universal-newline line splitting (`\r\n` is one boundary; the set is
    # upstream's `\t\n\v\f\r\x1c\x1d\x1e\x85\u2028\u2029`); no
    # trailing empty line.
    def splitlines(self, keepends: Bool = False) -> List[String]:
        var lines = List[String]()
        var line_start = 0
        var at = 0
        while at < self._size:
            var width = self._newline_width_at(at)
            if width == 0:
                at += self._lead_width(Int(self._data[at]))
                continue
            var end = at + width if keepends else at
            lines.append(self._with_bytes(line_start, end - line_start))
            at += width
            line_start = at
        if line_start < self._size:
            lines.append(self._with_bytes(line_start, self._size - line_start))
        return lines^

    # Case conversion over the full Unicode 16 simple and SpecialCasing
    # tables (upstream `to_uppercase`/`to_lowercase`); the table views are
    # built once per call.
    def upper(self) -> String:
        var upper = _upper_table()
        var upper2 = _upper2_table()
        var upper3 = _upper3_table()
        var result = String()
        var at = 0
        while at < self._size:
            var width = self._width_at(at)
            var scalar = self._scalar_at(at, width)
            _append_uppercased(result, upper, upper2, upper3, self, at, width, scalar)
            at += width
        return result^

    def lower(self) -> String:
        var lower = _lower_table()
        var result = String()
        var at = 0
        while at < self._size:
            var width = self._width_at(at)
            var scalar = self._scalar_at(at, width)
            var mapped = _lower_mapping(lower, scalar)
            if mapped == scalar:
                result._append_bytes_of(self, at, width)
            else:
                _append_scalar(result, mapped)
            at += width
        return result^

    # Upstream's rule: at least one cased character, and no character of
    # the other case (a character is uppercase when it has a lowercase
    # mapping, lowercase when it has an uppercase mapping).
    def isupper(self) -> Bool:
        return self._size > 0 and self._all_cased_as(True)

    def islower(self) -> Bool:
        return self._size > 0 and self._all_cased_as(False)

    # Non-empty and made only of Python-space codepoints (the audited head
    # declares no `single_character` fast-path parameter either).
    def isspace(self) -> Bool:
        var at = 0
        while at < self._size:
            var width = self._space_width_at(at)
            if width == 0:
                return False
            at += width
        return self._size > 0

    def is_ascii_digit(self) -> Bool:
        var i = 0
        while i < self._size:
            var b = Int(self._data[i])
            if b < 48 or b > 57:
                return False
            i += 1
        return self._size > 0

    def is_ascii_printable(self) -> Bool:
        var i = 0
        while i < self._size:
            var b = Int(self._data[i])
            if b < 32 or b > 126:
                return False
            i += 1
        return True

    # Byte-width justification with a one-byte fill character (upstream's
    # `ascii_*` family); a string at least `width` bytes long is returned
    # unchanged, and center puts the extra fill byte on the right.
    def ascii_rjust(self, width: Int, fillchar: StringSpan = " ") -> String:
        return self._justify(width - self._size, width, fillchar)

    def ascii_ljust(self, width: Int, fillchar: StringSpan = " ") -> String:
        return self._justify(0, width, fillchar)

    def ascii_center(self, width: Int, fillchar: StringSpan = " ") -> String:
        return self._justify((width - self._size) >> 1, width, fillchar)

    # The strip family and affix removal return sub-views of this buffer.
    # The default set is POSIX space (`" \t\n\v\f\r\x1c\x1d\x1e"`); the
    # `chars` form strips by codepoint membership in `chars`.
    def strip(self) -> Self:
        var start = self._lstrip_bound(0, self._size)
        return self._sub_view(start, self._rstrip_bound(start, self._size))

    def strip(self, chars: StringSpan) -> Self:
        var start = self._lstrip_chars_bound(chars, 0, self._size)
        return self._sub_view(start, self._rstrip_chars_bound(chars, start, self._size))

    def lstrip(self) -> Self:
        return self._sub_view(self._lstrip_bound(0, self._size), self._size)

    def lstrip(self, chars: StringSpan) -> Self:
        return self._sub_view(self._lstrip_chars_bound(chars, 0, self._size), self._size)

    def rstrip(self) -> Self:
        return self._sub_view(0, self._rstrip_bound(0, self._size))

    def rstrip(self, chars: StringSpan) -> Self:
        return self._sub_view(0, self._rstrip_chars_bound(chars, 0, self._size))

    def removeprefix(self, prefix: StringSpan, /) -> Self:
        if self.startswith(prefix):
            return self._sub_view(prefix._size, self._size)
        return self

    def removesuffix(self, suffix: StringSpan, /) -> Self:
        if suffix._size > 0 and self.endswith(suffix):
            return self._sub_view(0, self._size - suffix._size)
        return self

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_bytes(self.as_bytes())

    def as_bytes(self) -> Span[Byte, Self.origin]:
        return Span[Byte](unsafe_ptr=self._data, length=self._size)

    def codepoints(self) -> _CodepointIter[Self.origin]:
        return _CodepointIter(self, 0)

    def codepoint_slices(self) -> _CodepointSliceIter[Self.origin]:
        return _CodepointSliceIter(self, 0)

    def graphemes(self) -> _GraphemeIter[Self.origin]:
        return _GraphemeIter(self, 0)

    def to_string(self) -> String:
        var result = String("")
        result.data.unsafe_free()
        result.data = unsafe_alloc[Byte](self._size)
        result.size = self._size
        result.cap = self._size
        var i = 0
        while i < self._size:
            result.data[i] = self._data[i]
            i += 1
        return result^

    def __getitem__(self, *, byte: Int) raises -> Byte:
        if byte < 0:
            raise Error("StringSpan byte index out of range")
        if byte >= self._size:
            raise Error("StringSpan byte index out of range")
        return self._data[byte]

    def __getitem__(self, *, codepoint: Int) raises -> Codepoint:
        if codepoint < 0:
            raise Error("StringSpan codepoint index out of range")
        var index = 0
        var seen = 0
        while index < self._size:
            var width = self._width_at(index)
            if seen == codepoint:
                var text = self._with_bytes(index, width)
                return Codepoint(self._scalar_at(index, width), text: text^)
            seen += 1
            index += width
        raise Error("StringSpan codepoint index out of range")

    def __getitem__(self, *, grapheme: Int) raises -> String:
        if grapheme < 0:
            raise Error("StringSpan grapheme index out of range")
        var index = 0
        var seen = 0
        while index < self._size:
            var end = self._next_grapheme_end(index)
            if seen == grapheme:
                return self._with_bytes(index, end - index)
            seen += 1
            index = end
        raise Error("StringSpan grapheme index out of range")

    # Non-raising counts (upstream): one codepoint per non-continuation
    # byte, and one extended grapheme cluster per forward scan step.
    def count_codepoints(self) -> Int:
        return self._codepoints_before(self._size)

    def count_graphemes(self) -> Int:
        return self._graphemes_between(0, self._size)

    def __reversed__(self) -> _GraphemeReversedIter[Self.origin]:
        return _GraphemeReversedIter(self, self._size, 0, False)

    def codepoint_slices_reversed(self) -> _CodepointSliceReversedIter[Self.origin]:
        return _CodepointSliceReversedIter(self, self._size)

    def graphemes_reversed(self) -> _GraphemeReversedIter[Self.origin]:
        return _GraphemeReversedIter(self, self._size, 0, False)

    def bytes(self) -> _BytesIter[Self.origin]:
        return _BytesIter(self, 0)

    # The views before and after the `n`-th grapheme-cluster boundary
    # (`n == 0` yields `("", self)`, `n` past the end `(self, "")`).
    def split_at_grapheme(
        self, n: Int
    ) -> Tuple[StringSpan[Self.origin], StringSpan[Self.origin]]:
        if n < 0:
            _mojito_abort("grapheme split index must be non-negative")
        var at = 0
        var seen = 0
        while seen < n and at < self._size:
            at = self._next_grapheme_end(at)
            seen += 1
        return (self._sub_view(0, at), self._sub_view(at, self._size))

    def __getitem__(self, *, byte: ContiguousSlice) -> Self:
        var start = byte.start.or_else(0)
        var end = byte.end.or_else(self._size)
        check_slice_bounds(start, end, self._size)
        if not self._boundary(start):
            _mojito_abort("StringSpan byte slice endpoint is not a codepoint boundary")
        if not self._boundary(end):
            _mojito_abort("StringSpan byte slice endpoint is not a codepoint boundary")
        return self._sub_view(start, end)

    def __getitem__(self, *, codepoint: ContiguousSlice) -> Self:
        var total = self.count_codepoints()
        var start = codepoint.start.or_else(0)
        var end = codepoint.end.or_else(total)
        check_slice_bounds(start, end, total)
        return self._sub_view(self._codepoint_offset(start), self._codepoint_offset(end))

    def __getitem__(self, *, grapheme: ContiguousSlice) -> Self:
        var total = self.count_graphemes()
        var start = grapheme.start.or_else(0)
        var end = grapheme.end.or_else(total)
        check_slice_bounds(start, end, total)
        return self._sub_view(self._grapheme_offset(start), self._grapheme_offset(end))

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.to_string())

    def write_repr_to(self, mut writer: Some[Writer]):
        self.to_string().write_repr_to(writer)

    # A provenance-preserving sub-view over `[start, end)` of this buffer.
    def _sub_view(self, start: Int, end: Int) -> Self:
        var view = self
        view._data = view._data.unsafe_offset(start)
        view._size = end - start
        return view^

    # Whether `offset` falls between UTF-8 sequences (or at either end).
    def _boundary(self, offset: Int) -> Bool:
        if offset == 0 or offset == self._size:
            return True
        var b = Int(self._data[offset])
        if b < 128:
            return True
        return b >= 192

    def _split_on(self, sep: StringSpan, maxsplit: Int) -> List[String]:
        var parts = List[String]()
        if sep._size == 0:
            parts.append(String())
            var at = 0
            while at < self._size:
                var width = self._lead_width(Int(self._data[at]))
                parts.append(self._with_bytes(at, width))
                at += width
            parts.append(String())
            return parts^
        var start = 0
        var splits = 0
        var at = self._find_from(sep, 0) if maxsplit != 0 else -1
        while at >= 0:
            parts.append(self._with_bytes(start, at - start))
            start = at + sep._size
            splits += 1
            at = -1
            if maxsplit < 0 or splits < maxsplit:
                at = self._find_from(sep, start)
        parts.append(self._with_bytes(start, self._size - start))
        return parts^

    def _split_whitespace(self, maxsplit: Int) -> List[String]:
        var parts = List[String]()
        var at = 0
        var splits = 0
        while at < self._size:
            var width = self._space_width_at(at)
            if width > 0:
                at += width
                continue
            var end = at
            if maxsplit >= 0 and splits == maxsplit:
                end = self._size
            else:
                while end < self._size and self._space_width_at(end) == 0:
                    end += self._lead_width(Int(self._data[end]))
            parts.append(self._with_bytes(at, end - at))
            splits += 1
            at = end
        return parts^

    # Naive forward byte search from byte offset `start`: the offset of the
    # first match at or after it, or -1.
    def _find_from(self, sub: StringSpan, start: Int) -> Int:
        var at = start
        while at + sub._size <= self._size:
            if self._matches_at(sub, at):
                return at
            at += 1
        return -1

    # Whether `sub`'s bytes appear verbatim at byte offset `at`; the caller
    # keeps `at + sub._size` within the buffer.
    def _matches_at(self, sub: StringSpan, at: Int) -> Bool:
        var i = 0
        while i < sub._size:
            if Int(self._data[at + i]) != Int(sub._data[i]):
                return False
            i += 1
        return True

    # A negative search start counts from the end and clamps at 0.
    def _search_start(self, start: Int) -> Int:
        if start >= 0:
            return start
        var from_end = start + self._size
        return from_end if from_end > 0 else 0

    # An owned copy of bytes `[start, start + count)`.
    def _with_bytes(self, start: Int, count: Int) -> String:
        return self._sub_view(start, start + count).to_string()

    def _justify(self, start: Int, width: Int, fillchar: StringSpan) -> String:
        if self._size >= width:
            return self.to_string()
        if fillchar._size != 1:
            _mojito_abort("fill char needs to be a one byte literal")
        var result = String(capacity_bytes=width)
        var i = 0
        while i < start:
            result._append_bytes_of(fillchar, 0, 1)
            i += 1
        result._append_bytes_of(self, 0, self._size)
        while result.size < width:
            result._append_bytes_of(fillchar, 0, 1)
        return result^

    def _all_cased_as(self, upper: Bool) -> Bool:
        var lower_table = _lower_table()
        var upper_table = _upper_table()
        var upper2 = _upper2_table()
        var upper3 = _upper3_table()
        var found = False
        var at = 0
        while at < self._size:
            var width = self._width_at(at)
            var scalar = self._scalar_at(at, width)
            at += width
            var has_lower = _has_lower_mapping(lower_table, scalar)
            var has_upper = _has_upper_mapping(upper_table, upper2, upper3, scalar)
            if upper:
                if has_lower:
                    found = True
                elif has_upper:
                    return False
            else:
                if has_upper:
                    found = True
                elif has_lower:
                    return False
        return found

    def _codepoints_before(self, end: Int) -> Int:
        var count = 0
        var i = 0
        while i < end:
            if not self._is_continuation(Int(self._data[i])):
                count += 1
            i += 1
        return count

    def _graphemes_between(self, start: Int, end: Int) -> Int:
        var count = 0
        var at = start
        while at < end:
            at = self._next_grapheme_end(at)
            count += 1
        return count

    # The byte offset after `count` codepoints (strict: `count` must not
    # exceed the codepoint count).
    def _codepoint_offset(self, count: Int) -> Int:
        var index = 0
        var seen = 0
        while seen < count:
            if index >= self._size:
                _mojito_abort("StringSpan codepoint slice bounds out of range")
            index += self._width_at(index)
            seen += 1
        return index

    # The byte offset after `count` extended grapheme clusters (strict).
    def _grapheme_offset(self, count: Int) -> Int:
        var index = 0
        var seen = 0
        while seen < count:
            if index >= self._size:
                _mojito_abort("StringSpan grapheme slice bounds out of range")
            index = self._next_grapheme_end(index)
            seen += 1
        return index

    # The byte offset one past the extended grapheme cluster starting at
    # `start`: decode the first codepoint, then extend while the pair rules
    # join, tracking the run of consecutive regional indicators (class 7).
    def _next_grapheme_end(self, start: Int) -> Int:
        var index = start
        var width = self._width_at(index)
        var prev_class = self._grapheme_class(self._scalar_at(index, width))
        index += width
        var ri_run = 0
        if prev_class == 7:
            ri_run = 1
        while index < self._size:
            width = self._width_at(index)
            var next_class = self._grapheme_class(self._scalar_at(index, width))
            if not self._grapheme_joins(prev_class, next_class, ri_run):
                return index
            if next_class == 7:
                ri_run += 1
            else:
                ri_run = 0
            prev_class = next_class
            index += width
        return index

    # A boundary at or before `end` from which forward segmentation is
    # canonical: the start of the nearest preceding CR, LF, or Control
    # codepoint (a break always precedes one, GB5; a CR LF pair starts at
    # the CR, GB3), else the buffer start.
    def _safe_grapheme_start(self, end: Int) -> Int:
        var at = end
        while at > 0:
            var head = at - 1
            while head > 0 and self._is_continuation(Int(self._data[head])):
                head -= 1
            var cls = self._grapheme_class(self._scalar_at(head, self._width_at(head)))
            if cls == 2 and head > 0 and Int(self._data[head - 1]) == 13:
                return head - 1
            if cls == 1 or cls == 2 or cls == 3:
                return head
            at = head
        return 0

    # The start of the last extended grapheme cluster ending at `end`, found
    # by forward-scanning from `safe_start` (a `_safe_grapheme_start`).
    def _prev_grapheme_start(self, end: Int, safe_start: Int) -> Int:
        var at = safe_start
        var last = at
        while at < end:
            last = at
            at = self._next_grapheme_end(at)
        return last

    # Whether UAX #29 keeps `next_class` in the cluster after `prev_class`,
    # using the `_grapheme_class` codes.  `ri_run` is the count of consecutive
    # regional indicators ending at the previous codepoint.  GB11 is
    # simplified to "never break after ZWJ" (no Extended_Pictographic data);
    # GB9b (Prepend) is omitted.
    def _grapheme_joins(self, prev_class: Int, next_class: Int, ri_run: Int) -> Bool:
        # GB3: CR x LF.
        if prev_class == 1 and next_class == 2:
            return True
        # GB4/GB5: otherwise break around Control, CR, and LF.
        if prev_class == 3 or prev_class == 1 or prev_class == 2:
            return False
        if next_class == 3 or next_class == 1 or next_class == 2:
            return False
        # GB6: L x (L | V | LV | LVT).
        if prev_class == 8:
            if next_class == 8 or next_class == 9:
                return True
            if next_class == 11 or next_class == 12:
                return True
        # GB7: (LV | V) x (V | T).
        if prev_class == 11 or prev_class == 9:
            if next_class == 9 or next_class == 10:
                return True
        # GB8: (LVT | T) x T.
        if prev_class == 12 or prev_class == 10:
            if next_class == 10:
                return True
        # GB9/GB9a: x (Extend | ZWJ | SpacingMark).
        if next_class == 4 or next_class == 5 or next_class == 6:
            return True
        # GB11 simplified: ZWJ x anything.
        if prev_class == 5:
            return True
        # GB12/GB13: regional indicators join in pairs.
        if prev_class == 7 and next_class == 7:
            return ri_run % 2 == 1
        # GB999.
        return False

    # Grapheme_Cluster_Break class of `cp`: the documented essentials subset —
    # hand-maintained Control/Extend/SpacingMark ranges, regional indicators,
    # and fully arithmetic Hangul.  Class codes (comptime constants would echo
    # in the CLI's final-bindings listing, so the codes stay literal):
    #   0 Other, 1 CR, 2 LF, 3 Control, 4 Extend, 5 ZWJ, 6 SpacingMark,
    #   7 Regional_Indicator, 8 L, 9 V, 10 T, 11 LV, 12 LVT.
    # Unlisted codepoints are 0 (Other).
    def _grapheme_class(self, cp: Int) -> Int:
        if cp == 0x0D:
            return 1
        if cp == 0x0A:
            return 2
        # Control essentials (non-exhaustive): C0/C1, soft hyphen, zero-width
        # space, line/paragraph separators and directional formatting, word
        # joiner and invisible operators, byte-order mark.
        if cp < 0x20:
            return 3
        if cp >= 0x7F and cp <= 0x9F:
            return 3
        if cp == 0xAD or cp == 0x200B or cp == 0xFEFF:
            return 3
        if cp >= 0x2028 and cp <= 0x202E:
            return 3
        if cp >= 0x2060 and cp <= 0x2064:
            return 3
        if cp == 0x200D:
            return 5
        # Extend essentials (non-exhaustive): ZWNJ, combining-mark blocks for
        # Latin/Cyrillic/Hebrew/Arabic/Devanagari/Thai, combining diacritical
        # extensions/supplement, combining marks for symbols, variation
        # selectors (plus supplement), emoji skin-tone modifiers, and tags.
        if cp == 0x200C:
            return 4
        if cp >= 0x0300 and cp <= 0x036F:
            return 4
        if cp >= 0x0483 and cp <= 0x0489:
            return 4
        if cp >= 0x0591 and cp <= 0x05BD:
            return 4
        if cp == 0x05BF or cp == 0x05C7:
            return 4
        if cp >= 0x05C1 and cp <= 0x05C2:
            return 4
        if cp >= 0x05C4 and cp <= 0x05C5:
            return 4
        if cp >= 0x0610 and cp <= 0x061A:
            return 4
        if cp >= 0x064B and cp <= 0x065F:
            return 4
        if cp == 0x0670:
            return 4
        if cp >= 0x06D6 and cp <= 0x06DC:
            return 4
        if cp >= 0x0900 and cp <= 0x0902:
            return 4
        if cp == 0x093C or cp == 0x094D:
            return 4
        if cp >= 0x0941 and cp <= 0x0948:
            return 4
        if cp >= 0x0951 and cp <= 0x0957:
            return 4
        if cp == 0x0E31:
            return 4
        if cp >= 0x0E34 and cp <= 0x0E3A:
            return 4
        if cp >= 0x0E47 and cp <= 0x0E4E:
            return 4
        if cp >= 0x1AB0 and cp <= 0x1AFF:
            return 4
        if cp >= 0x1DC0 and cp <= 0x1DFF:
            return 4
        if cp >= 0x20D0 and cp <= 0x20FF:
            return 4
        if cp >= 0xFE00 and cp <= 0xFE0F:
            return 4
        if cp >= 0xFE20 and cp <= 0xFE2F:
            return 4
        if cp >= 0x1F3FB and cp <= 0x1F3FF:
            return 4
        if cp >= 0xE0020 and cp <= 0xE007F:
            return 4
        if cp >= 0xE0100 and cp <= 0xE01EF:
            return 4
        # SpacingMark essentials (non-exhaustive): Devanagari and Thai/Lao
        # spacing vowel signs.
        if cp == 0x0903 or cp == 0x093B:
            return 6
        if cp >= 0x093E and cp <= 0x0940:
            return 6
        if cp >= 0x0949 and cp <= 0x094C:
            return 6
        if cp >= 0x094E and cp <= 0x094F:
            return 6
        if cp == 0x0E33 or cp == 0x0EB3:
            return 6
        if cp >= 0x1F1E6 and cp <= 0x1F1FF:
            return 7
        # Hangul is fully arithmetic: conjoining jamo blocks and the
        # precomposed-syllable block, where LV syllables sit every 28 steps.
        if cp >= 0x1100 and cp <= 0x115F:
            return 8
        if cp >= 0xA960 and cp <= 0xA97C:
            return 8
        if cp >= 0x1160 and cp <= 0x11A7:
            return 9
        if cp >= 0xD7B0 and cp <= 0xD7C6:
            return 9
        if cp >= 0x11A8 and cp <= 0x11FF:
            return 10
        if cp >= 0xD7CB and cp <= 0xD7FB:
            return 10
        if cp >= 0xAC00 and cp <= 0xD7A3:
            if (cp - 0xAC00) % 28 == 0:
                return 11
            return 12
        return 0

    # Non-raising scalar decode of the `width`-byte sequence at `at`.
    def _scalar_at(self, at: Int, width: Int) -> Int:
        var lead = Int(self._data[at])
        if width == 1:
            return lead
        var value = lead % 32 if width == 2 else (lead % 16 if width == 3 else lead % 8)
        var i = 1
        while i < width:
            value = value * 64 + Int(self._data[at + i]) % 64
            i += 1
        return value

    # Non-raising UTF-8 lead-byte width (a stray continuation byte counts as
    # one so scans always advance).
    def _lead_width(self, lead: Int) -> Int:
        if lead < 224:
            return 1 if lead < 192 else 2
        return 3 if lead < 240 else 4

    # The width of the sequence at `at`, clamped to the buffer so a
    # truncated tail never reads past the end.
    def _width_at(self, at: Int) -> Int:
        var width = self._lead_width(Int(self._data[at]))
        if at + width > self._size:
            return self._size - at
        return width

    def _is_continuation(self, b: Int) -> Bool:
        return b >= 128 and b < 192

    def _is_posix_space_byte(self, b: Int) -> Bool:
        return b == 32 or (b >= 9 and b <= 13) or (b >= 28 and b <= 30)

    # The byte width of the Python-space codepoint at byte offset `at`, or 0.
    def _space_width_at(self, at: Int) -> Int:
        var b = Int(self._data[at])
        if self._is_posix_space_byte(b):
            return 1
        return self._unicode_separator_width_at(at)

    # The byte width of the line boundary at byte offset `at` (`\r\n` counts
    # as one), or 0.
    def _newline_width_at(self, at: Int) -> Int:
        var b = Int(self._data[at])
        if b == 13:
            if at + 1 < self._size and Int(self._data[at + 1]) == 10:
                return 2
            return 1
        if (b >= 9 and b <= 13) or (b >= 28 and b <= 30):
            return 1
        return self._unicode_separator_width_at(at)

    # U+0085 (C2 85), U+2028 (E2 80 A8), and U+2029 (E2 80 A9).
    def _unicode_separator_width_at(self, at: Int) -> Int:
        var b = Int(self._data[at])
        if b == 0xC2 and at + 1 < self._size and Int(self._data[at + 1]) == 0x85:
            return 2
        if b == 0xE2 and at + 2 < self._size and Int(self._data[at + 1]) == 0x80:
            var b2 = Int(self._data[at + 2])
            if b2 == 0xA8 or b2 == 0xA9:
                return 3
        return 0

    def _lstrip_bound(self, start: Int, end: Int) -> Int:
        var at = start
        while at < end and self._is_posix_space_byte(Int(self._data[at])):
            at += 1
        return at

    def _rstrip_bound(self, start: Int, end: Int) -> Int:
        var at = end
        while at > start and self._is_posix_space_byte(Int(self._data[at - 1])):
            at -= 1
        return at

    def _lstrip_chars_bound(self, chars: StringSpan, start: Int, end: Int) -> Int:
        var at = start
        while at < end:
            var width = self._lead_width(Int(self._data[at]))
            if not chars._has_sequence(self, at, width):
                break
            at += width
        return at

    def _rstrip_chars_bound(self, chars: StringSpan, start: Int, end: Int) -> Int:
        var at = end
        while at > start:
            var head = at - 1
            while head > start and self._is_continuation(Int(self._data[head])):
                head -= 1
            if not chars._has_sequence(self, head, at - head):
                break
            at = head
        return at

    # Whether the `width` bytes of `other` at `at` occur in this buffer.  In
    # valid UTF-8 a whole sequence matches only at a codepoint boundary, so
    # this is codepoint membership.
    def _has_sequence(self, other: StringSpan, at: Int, width: Int) -> Bool:
        var pos = 0
        while pos + width <= self._size:
            var i = 0
            while i < width and Int(self._data[pos + i]) == Int(other._data[at + i]):
                i += 1
            if i == width:
                return True
            pos += 1
        return False


# The grapheme-cluster iterator behind ordinary String/StringSpan
# iteration: each step yields the next extended grapheme cluster as a
# borrowed StringSpan sub-view of the source buffer. The origin parameters
# stay erased on the bundled template (like `_ListIter`); the loop site
# retains the source loan through the iteration protocol.
@fieldwise_init
struct _GraphemeIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = StringSpan[Self.iterable_origin]

    var src: StringSpan[Self.iterable_origin]
    var index: Int

    # An iterator is its own iterable (`for x in s.graphemes()`).
    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.index >= self.src.byte_length():
            raise StopIteration()
        var start = self.index
        var end = self.src._next_grapheme_end(start)
        self.index = end
        return self.src._sub_view(start, end)

    # Remaining grapheme clusters (`Sized`, as upstream's iterator).
    def __len__(self) -> Int:
        return self.src._graphemes_between(self.index, self.src.byte_length())


# `graphemes_reversed()` / `reversed(s)`: the clusters back to front. The
# UAX #29 rules scan forward, so each step forward-scans from a cached safe
# boundary (`_safe_grapheme_start`) to the cluster ending at `end`.
@fieldwise_init
struct _GraphemeReversedIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = StringSpan[Self.iterable_origin]

    var src: StringSpan[Self.iterable_origin]
    var end: Int
    var safe_start: Int
    var safe_known: Bool

    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.end <= 0:
            raise StopIteration()
        if not self.safe_known or self.safe_start >= self.end:
            self.safe_start = self.src._safe_grapheme_start(self.end)
            self.safe_known = True
        var start = self.src._prev_grapheme_start(self.end, self.safe_start)
        var end = self.end
        self.end = start
        return self.src._sub_view(start, end)

    def __len__(self) -> Int:
        return self.src._graphemes_between(0, self.end)


# `String.codepoints()`: decoded `Codepoint` values over a borrowed view.
@fieldwise_init
struct _CodepointIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = Codepoint

    var src: StringSpan[Self.iterable_origin]
    var index: Int

    # An iterator is its own iterable (`for x in s.codepoints()`).
    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> Codepoint:
        if self.index >= self.src.byte_length():
            raise StopIteration()
        var width = self.src._width_at(self.index)
        var scalar = self.src._scalar_at(self.index, width)
        var piece = self.src._with_bytes(self.index, width)
        self.index += width
        return Codepoint(scalar, text: piece^)

    # The next codepoint without advancing, or None at the end.
    def peek_next(self) -> Optional[Codepoint]:
        if self.index >= self.src.byte_length():
            return None
        var width = self.src._width_at(self.index)
        var piece = self.src._with_bytes(self.index, width)
        var value = Codepoint(self.src._scalar_at(self.index, width), text: piece^)
        return Optional[Codepoint](value^)

    def __len__(self) -> Int:
        var total = self.src._codepoints_before(self.src.byte_length())
        return total - self.src._codepoints_before(self.index)


# `String.codepoint_slices()`: one-codepoint sub-views of the source buffer.
@fieldwise_init
struct _CodepointSliceIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = StringSpan[Self.iterable_origin]

    var src: StringSpan[Self.iterable_origin]
    var index: Int

    # An iterator is its own iterable (`for x in s.codepoint_slices()`).
    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.index >= self.src.byte_length():
            raise StopIteration()
        var start = self.index
        var end = start + self.src._width_at(start)
        self.index = end
        return self.src._sub_view(start, end)

    # The next codepoint's view without advancing, or None at the end.
    def peek_next(self) -> Optional[StringSpan[Self.iterable_origin]]:
        if self.index >= self.src.byte_length():
            return None
        return self.src._sub_view(self.index, self.index + self.src._width_at(self.index))

    def __len__(self) -> Int:
        var total = self.src._codepoints_before(self.src.byte_length())
        return total - self.src._codepoints_before(self.index)


# `codepoint_slices_reversed()`: the one-codepoint sub-views back to front.
@fieldwise_init
struct _CodepointSliceReversedIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = StringSpan[Self.iterable_origin]

    var src: StringSpan[Self.iterable_origin]
    var end: Int

    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.end <= 0:
            raise StopIteration()
        var start = self.end - 1
        while start > 0 and self.src._is_continuation(Int(self.src._data[start])):
            start -= 1
        var end = self.end
        self.end = start
        return self.src._sub_view(start, end)

    def __len__(self) -> Int:
        return self.src._codepoints_before(self.end)


# `bytes()`: the raw UTF-8 bytes of a borrowed view.
@fieldwise_init
struct _BytesIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = Byte

    var src: StringSpan[Self.iterable_origin]
    var index: Int

    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> Byte:
        if self.index >= self.src.byte_length():
            raise StopIteration()
        var at = self.index
        self.index += 1
        return self.src._data[at]

    def __len__(self) -> Int:
        return self.src.byte_length() - self.index



# Upstream's `CStringSlice` (std.ffi, re-exported there): a view over the
# NUL-terminated bytes of a String, the argument C-string parameters take.
# `String.as_c_string_slice()` writes the terminator and mints the view; the
# constructor itself does not, so it is not `@implicit`. Mojito views `Byte`
# rather than upstream's `c_char` (`Int8`) elements.
struct CStringSlice[mut: Bool, //, origin: Origin[mut=mut]](
    ImplicitlyCopyable, Movable, Writable
):
    var _data: Pointer[Byte, Self.origin._get_owned_interior["bytes"]]

    def __init__(out self, ref [Self.origin] src: String):
        self._data = src.data.unsafe_origin_cast[
            origin._get_owned_interior["bytes"]
        ]()

    def unsafe_ptr(self) -> Pointer[Byte, Self.origin._get_owned_interior["bytes"]]:
        return self._data

    def byte_length(self) -> Int:
        return Int(external_call["strlen", UInt](self._data))

    def __len__(self) -> Int:
        return self.byte_length()

    def write_to(self, mut writer: Some[Writer]):
        writer.write(String(unsafe_from_utf8_ptr=self._data))
