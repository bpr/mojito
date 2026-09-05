# A self-hosted UTF-8 String.  `data` owns `size` initialized bytes in a
# `cap`-byte allocation.  Construction from a literal is the compiler's
# literal-to-struct bridge (the byte buffer is filled from the literal's
# UTF-8 bytes at the call); every other operation is ordinary library code
# over the byte buffer.  Slicing and the result APIs (`find`/`rfind`/
# `startswith`/`endswith`/`split`) work in byte offsets, like `len`.
#
# `s[codepoint=i]` yields a `Codepoint` value carrying both the decoded
# scalar and the character's text.  `s[grapheme=i]` and `count_graphemes()`
# segment extended grapheme clusters with a documented UAX #29 subset: a
# hand-maintained essentials classifier plus arithmetic Hangul, with GB11
# simplified to "never break after ZWJ" and GB9b (Prepend) omitted.

from std.memory import unsafe_alloc

from std.collections.list import List
from std.optional import Optional
from std.span import Span

from std.iterable import Iterable, Iterator, StopIteration

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


# Simple case mappings for the bundled subset: ASCII, Latin-1 Supplement,
# Latin Extended-A (alternating pairs, with the `ı`/`ſ` specials), Greek
# (final sigma folds to capital sigma), and Cyrillic (including the
# `Ѐ`-`Џ` row). Every other scalar maps to itself.
def _upper_scalar(cp: Int) -> Int:
    if cp >= 0x61 and cp <= 0x7A:
        return cp - 0x20
    if cp < 0xE0:
        return cp
    if cp <= 0xFE:
        return cp if cp == 0xF7 else cp - 0x20
    if cp == 0xFF:
        return 0x178
    if cp >= 0x100 and cp <= 0x137:
        if cp == 0x131:
            return 0x49
        return cp - 1 if cp % 2 == 1 else cp
    if cp >= 0x139 and cp <= 0x148:
        return cp - 1 if cp % 2 == 0 else cp
    if cp >= 0x14A and cp <= 0x177:
        return cp - 1 if cp % 2 == 1 else cp
    if cp >= 0x17A and cp <= 0x17E:
        return cp - 1 if cp % 2 == 0 else cp
    if cp == 0x17F:
        return 0x53
    if cp >= 0x3B1 and cp <= 0x3C9:
        return 0x3A3 if cp == 0x3C2 else cp - 0x20
    if cp >= 0x430 and cp <= 0x44F:
        return cp - 0x20
    if cp >= 0x450 and cp <= 0x45F:
        return cp - 0x50
    return cp


def _lower_scalar(cp: Int) -> Int:
    if cp >= 0x41 and cp <= 0x5A:
        return cp + 0x20
    if cp < 0xC0:
        return cp
    if cp <= 0xDE:
        return cp if cp == 0xD7 else cp + 0x20
    if cp == 0x178:
        return 0xFF
    if cp >= 0x100 and cp <= 0x137:
        if cp == 0x130:
            return 0x69
        return cp + 1 if cp % 2 == 0 else cp
    if cp >= 0x139 and cp <= 0x148:
        return cp + 1 if cp % 2 == 1 else cp
    if cp >= 0x14A and cp <= 0x177:
        return cp + 1 if cp % 2 == 0 else cp
    if cp >= 0x179 and cp <= 0x17E:
        return cp + 1 if cp % 2 == 1 else cp
    if cp >= 0x391 and cp <= 0x3A9:
        return cp if cp == 0x3A2 else cp + 0x20
    if cp >= 0x410 and cp <= 0x42F:
        return cp + 0x20
    if cp >= 0x400 and cp <= 0x40F:
        return cp + 0x50
    return cp


# Message helpers return fresh temporaries: the parsers keep no heap-owning
# local live at a raise site (see the native raise-path residue in
# docs/roadmap.md).
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


# Floating-point parsing (upstream `atof`'s decimal core): POSIX-space
# padding, an optional sign, `inf`/`nan`, and a digits[.digits][e[+-]digits]
# body evaluated as an integer significand scaled by an exact power of ten
# (correctly rounded while the significand and the scale stay exact; the
# extended-precision fallback for longer inputs is a recorded gap).
# 1 for `nan`, 2 for `inf`/`infinity` (case-insensitive), else 0.
def _float_special(str: String, start: Int, end: Int) -> Int:
    var lowered = str._with_bytes(start, end - start).lower()
    if lowered == "nan":
        return 1
    if lowered == "inf" or lowered == "infinity":
        return 2
    return 0


def _float_error(str: String) -> String:
    return "String is not convertible to float: '" + str + "'"


def _float_edge_error(str: String, start: Int, end: Int, which: String) -> String:
    return (
        _float_error(str) + ". The " + which + " character of '"
        + str._with_bytes(start, end - start)
        + "' should be a digit or dot to convert it to a float."
    )


def atof(str: String) raises -> Float64:
    if str.size == 0 or (str.size == 1 and Int(str.data[0]) == 46):
        raise Error(_float_error(str))
    var start = 0
    var end = str.size
    while start < end and str._is_posix_space_byte(Int(str.data[start])):
        start += 1
    while end > start and str._is_posix_space_byte(Int(str.data[end - 1])):
        end -= 1
    var sign = 1.0
    if start < end and (Int(str.data[start]) == 43 or Int(str.data[start]) == 45):
        if Int(str.data[start]) == 45:
            sign = -1.0
        start += 1
    var special = _float_special(str, start, end)
    if special == 1:
        return _float_nan()
    if special == 2:
        return _float_inf() * sign
    if start >= end:
        raise Error(_float_error(str))
    var first = Int(str.data[start])
    if not ((first >= 48 and first <= 57) or first == 46):
        raise Error(_float_edge_error(str, start, end, "first"))
    var last = Int(str.data[end - 1])
    if not ((last >= 48 and last <= 57) or last == 46):
        raise Error(_float_edge_error(str, start, end, "last"))
    var significand = 0
    var digits = 0
    var scale = 0
    var seen_dot = False
    var exponent = 0
    var exponent_sign = 1
    var in_exponent = False
    var i = start
    while i < end:
        var b = Int(str.data[i])
        if b >= 48 and b <= 57:
            if in_exponent:
                exponent = exponent * 10 + (b - 48)
            else:
                if digits < 19:
                    significand = significand * 10 + (b - 48)
                    digits += 1
                    if seen_dot:
                        scale -= 1
                elif not seen_dot:
                    scale += 1
        elif b == 46 and not seen_dot and not in_exponent:
            seen_dot = True
        elif (b == 101 or b == 69) and not in_exponent:
            in_exponent = True
            if i + 1 < end and (Int(str.data[i + 1]) == 43 or Int(str.data[i + 1]) == 45):
                if Int(str.data[i + 1]) == 45:
                    exponent_sign = -1
                i += 1
        else:
            raise Error(
                _float_error(str) + ". Invalid character(s) in the number: '"
                + str._with_bytes(start, end - start) + "'"
            )
        i += 1
    var power = scale + exponent_sign * exponent
    var value = Float64(significand)
    if power > 0:
        value = value * _pow10(power)
    elif power < 0:
        value = value / _pow10(-power)
    return value * sign


struct String(
    Boolable,
    Comparable,
    Copyable,
    Equatable,
    Hashable,
    ImplicitlyCopyable,
    Iterable,
    Movable,
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
            new_data[i] = self.data[i]
            i += 1
        self.data.unsafe_free()
        self.data = new_data
        self.cap = new_capacity_bytes

    def __len__(self) -> Int:
        return self.size

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

    def __contains__(self, sub: String) -> Bool:
        return self._find_from(sub, 0) >= 0

    # Result APIs use byte offsets, like `len` and the byte-wise slice
    # (upstream `string_span.mojo`).  An empty needle matches everywhere
    # (Python semantics): `find` reports 0, `rfind` the byte length, the
    # affix tests True.  A negative `start` counts from the end and clamps.

    def find(self, substr: String, start: Int = 0) -> Int:
        if substr.size == 0:
            return 0
        if self.size < substr.size + start:
            return -1
        return self._find_from(substr, self._search_start(start))

    def rfind(self, substr: String, start: Int = 0) -> Int:
        if substr.size == 0:
            return self.size
        if self.size < substr.size + start:
            return -1
        var start_byte = self._search_start(start)
        var at = self.size - substr.size
        while at >= start_byte:
            if self._matches_at(substr, at):
                return at
            at -= 1
        return -1

    def count(self, substr: String) -> Int:
        if substr.size == 0:
            return self.size + 1
        var total = 0
        var at = self._find_from(substr, 0)
        while at >= 0:
            total += 1
            at = self._find_from(substr, at + substr.size)
        return total

    # `start`/`end` are byte offsets; `end == -1` means the whole string.
    def startswith(self, prefix: String, start: Int = 0, end: Int = -1) -> Bool:
        if end == -1:
            return self.find(prefix, start) == start
        if start < 0 or end > self.size or end - start < prefix.size:
            return False
        return self._matches_at(prefix, start)

    def endswith(self, suffix: String, start: Int = 0, end: Int = -1) -> Bool:
        if suffix.size > self.size:
            return False
        if end == -1:
            return self.rfind(suffix, start) + suffix.size == self.size
        if start < 0 or end > self.size or end - start < suffix.size:
            return False
        return self._matches_at(suffix, end - suffix.size)

    # Every occurrence of `old` replaced by `new`; an empty `old` interleaves
    # `new` before every codepoint.
    def replace(self, old: String, new: String) -> String:
        var result = String()
        if old.size == 0:
            var at = 0
            while at < self.size:
                var width = self._lead_width(Int(self.data[at]))
                result._append_bytes_of(new, 0, new.size)
                result._append_bytes_of(self, at, width)
                at += width
            return result^
        var start = 0
        var at = self._find_from(old, 0)
        while at >= 0:
            result._append_bytes_of(self, start, at - start)
            result._append_bytes_of(new, 0, new.size)
            start = at + old.size
            at = self._find_from(old, start)
        result._append_bytes_of(self, start, self.size - start)
        return result^

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

    # Eager owned pieces rather than current Mojo's borrowed StringSlice
    # views (the recorded eager-result divergence).  An empty separator
    # yields an empty piece, every codepoint, and an empty piece (upstream
    # ignores `maxsplit` there); `maxsplit` bounds the number of splits.
    def split(self, sep: String, maxsplit: Int = -1) -> List[String]:
        var parts = List[String]()
        if sep.size == 0:
            parts.append(String())
            var at = 0
            while at < self.size:
                var width = self._lead_width(Int(self.data[at]))
                parts.append(self._with_bytes(at, width))
                at += width
            parts.append(String())
            return parts^
        var start = 0
        var splits = 0
        var at = self._find_from(sep, 0) if maxsplit != 0 else -1
        while at >= 0:
            parts.append(self._with_bytes(start, at - start))
            start = at + sep.size
            splits += 1
            at = -1
            if maxsplit < 0 or splits < maxsplit:
                at = self._find_from(sep, start)
        parts.append(self._with_bytes(start, self.size - start))
        return parts^

    # Whitespace split: runs of Python-space codepoints (POSIX space plus
    # U+0085, U+2028, U+2029) separate pieces and never yield empty ones.
    def split(self, sep: NoneType = None, *, maxsplit: Int = -1) -> List[String]:
        var parts = List[String]()
        var at = 0
        var splits = 0
        while at < self.size:
            var width = self._space_width_at(at)
            if width > 0:
                at += width
                continue
            var end = at
            if maxsplit >= 0 and splits == maxsplit:
                end = self.size
            else:
                while end < self.size and self._space_width_at(end) == 0:
                    end += self._lead_width(Int(self.data[end]))
            parts.append(self._with_bytes(at, end - at))
            splits += 1
            at = end
        return parts^

    # Universal-newline line splitting (`\r\n` is one boundary; the set is
    # upstream's `\t\n\v\f\r\x1c\x1d\x1e\x85\u2028\u2029`); no
    # trailing empty line.
    def splitlines(self, keepends: Bool = False) -> List[String]:
        var lines = List[String]()
        var line_start = 0
        var at = 0
        while at < self.size:
            var width = self._newline_width_at(at)
            if width == 0:
                at += self._lead_width(Int(self.data[at]))
                continue
            var end = at + width if keepends else at
            lines.append(self._with_bytes(line_start, end - line_start))
            at += width
            line_start = at
        if line_start < self.size:
            lines.append(self._with_bytes(line_start, self.size - line_start))
        return lines^

    # Case conversion over a pure-Mojo simple-case subset: ASCII, Latin-1,
    # Latin Extended-A, Greek, and Cyrillic letters (plus `ß` -> `SS`);
    # other scripts pass through unchanged (upstream maps the full Unicode
    # tables).
    def upper(self) -> String:
        var result = String()
        var at = 0
        while at < self.size:
            var width = self._lead_width(Int(self.data[at]))
            var scalar = self._scalar_at(at, width)
            if scalar == 0xDF:
                var double_s = String("SS")
                result._append_bytes_of(double_s, 0, 2)
            else:
                var mapped = _upper_scalar(scalar)
                if mapped == scalar:
                    result._append_bytes_of(self, at, width)
                else:
                    var text = Codepoint._encode_utf8(mapped)
                    result._append_bytes_of(text, 0, text.size)
            at += width
        return result^

    def lower(self) -> String:
        var result = String()
        var at = 0
        while at < self.size:
            var width = self._lead_width(Int(self.data[at]))
            var scalar = self._scalar_at(at, width)
            var mapped = _lower_scalar(scalar)
            if mapped == scalar:
                result._append_bytes_of(self, at, width)
            else:
                var text = Codepoint._encode_utf8(mapped)
                result._append_bytes_of(text, 0, text.size)
            at += width
        return result^

    # Upstream's rule: at least one cased character, and no character of
    # the other case (a character is uppercase when it has a lowercase
    # mapping, lowercase when it has an uppercase mapping).
    def isupper(self) -> Bool:
        return self.size > 0 and self._all_cased_as(True)

    def islower(self) -> Bool:
        return self.size > 0 and self._all_cased_as(False)

    # Non-empty and made only of Python-space codepoints.
    def isspace(self) -> Bool:
        var at = 0
        while at < self.size:
            var width = self._space_width_at(at)
            if width == 0:
                return False
            at += width
        return self.size > 0

    def is_ascii_digit(self) -> Bool:
        var i = 0
        while i < self.size:
            var b = Int(self.data[i])
            if b < 48 or b > 57:
                return False
            i += 1
        return self.size > 0

    def is_ascii_printable(self) -> Bool:
        var i = 0
        while i < self.size:
            var b = Int(self.data[i])
            if b < 32 or b > 126:
                return False
            i += 1
        return True

    # Byte-width justification with a one-byte fill character (upstream's
    # `ascii_*` family); a string at least `width` bytes long is returned
    # unchanged, and center puts the extra fill byte on the right.
    def ascii_rjust(self, width: Int, fillchar: String = " ") -> String:
        return self._justify(width - self.size, width, fillchar)

    def ascii_ljust(self, width: Int, fillchar: String = " ") -> String:
        return self._justify(0, width, fillchar)

    def ascii_center(self, width: Int, fillchar: String = " ") -> String:
        return self._justify((width - self.size) >> 1, width, fillchar)

    # Byte access: a borrowed `Span[Byte]` over the buffer and the raw
    # interior pointer (current Mojo's `as_bytes`/`unsafe_ptr`).
    def as_bytes(ref self) -> Span[Byte, origin_of(self)]:
        return Span[Byte](unsafe_ptr=self.data, length=self.size)

    def unsafe_ptr(ref self) -> Pointer[Byte, origin_of(self)._get_owned_interior["bytes"]]:
        return self.data.unsafe_origin_cast[origin_of(self)._get_owned_interior["bytes"]]()

    def capacity_bytes(self) -> Int:
        return self.cap

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

    # The strip family returns borrowed views of this buffer.  The default
    # set is POSIX space (`" \t\n\v\f\r\x1c\x1d\x1e"`); the `chars` form
    # strips by codepoint membership in `chars`.
    def strip(self) -> StringSpan[origin_of(self)]:
        var start = self._lstrip_bound(0, self.size)
        return self._byte_view(start, self._rstrip_bound(start, self.size))

    def strip(self, chars: String) -> StringSpan[origin_of(self)]:
        var start = self._lstrip_chars_bound(chars, 0, self.size)
        return self._byte_view(start, self._rstrip_chars_bound(chars, start, self.size))

    def lstrip(self) -> StringSpan[origin_of(self)]:
        return self._byte_view(self._lstrip_bound(0, self.size), self.size)

    def lstrip(self, chars: String) -> StringSpan[origin_of(self)]:
        return self._byte_view(self._lstrip_chars_bound(chars, 0, self.size), self.size)

    def rstrip(self) -> StringSpan[origin_of(self)]:
        return self._byte_view(0, self._rstrip_bound(0, self.size))

    def rstrip(self, chars: String) -> StringSpan[origin_of(self)]:
        return self._byte_view(0, self._rstrip_chars_bound(chars, 0, self.size))

    def removeprefix(self, prefix: String, /) -> StringSpan[origin_of(self)]:
        if self.startswith(prefix):
            return self._byte_view(prefix.size, self.size)
        return self._byte_view(0, self.size)

    def removesuffix(self, suffix: String, /) -> StringSpan[origin_of(self)]:
        if suffix.size > 0 and self.endswith(suffix):
            return self._byte_view(0, self.size - suffix.size)
        return self._byte_view(0, self.size)

    # Naive forward byte search from byte offset `start`: the offset of the
    # first match at or after it, or -1.
    def _find_from(self, sub: String, start: Int) -> Int:
        var at = start
        while at + sub.size <= self.size:
            if self._matches_at(sub, at):
                return at
            at += 1
        return -1

    # Whether `sub`'s bytes appear verbatim at byte offset `at`; the caller
    # keeps `at + sub.size` within the buffer.
    def _matches_at(self, sub: String, at: Int) -> Bool:
        var i = 0
        while i < sub.size:
            if Int(self.data[at + i]) != Int(sub.data[i]):
                return False
            i += 1
        return True

    # A negative search start counts from the end and clamps at 0.
    def _search_start(self, start: Int) -> Int:
        if start >= 0:
            return start
        var from_end = start + self.size
        return from_end if from_end > 0 else 0

    # Append `count` bytes of `src` from byte offset `start`, doubling the
    # capacity when the buffer is full.
    def _append_bytes_of(mut self, src: String, start: Int, count: Int):
        var needed = self.size + count
        if needed > self.cap:
            var new_cap = self.cap * 2
            if new_cap < needed:
                new_cap = needed
            self.reserve_bytes(new_cap)
        var i = 0
        while i < count:
            self.data[self.size + i] = src.data[start + i]
            i += 1
        self.size = needed

    # A borrowed view over bytes `[start, end)` of this buffer.
    def _byte_view(self, start: Int, end: Int) -> StringSpan[origin_of(self)]:
        var view = StringSpan(self)
        view._data = view._data.unsafe_offset(start)
        view._size = end - start
        return view^

    def _justify(self, start: Int, width: Int, fillchar: String) -> String:
        if self.size >= width:
            return self.copy()
        if fillchar.size != 1:
            _mojito_abort("fill char needs to be a one byte literal")
        var result = String(capacity_bytes=width)
        var i = 0
        while i < start:
            result._append_bytes_of(fillchar, 0, 1)
            i += 1
        result._append_bytes_of(self, 0, self.size)
        while result.size < width:
            result._append_bytes_of(fillchar, 0, 1)
        return result^

    def _all_cased_as(self, upper: Bool) -> Bool:
        var found = False
        var at = 0
        while at < self.size:
            var width = self._lead_width(Int(self.data[at]))
            var scalar = self._scalar_at(at, width)
            at += width
            var has_lower = _lower_scalar(scalar) != scalar
            var has_upper = _upper_scalar(scalar) != scalar or scalar == 0xDF
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

    # Non-raising scalar decode of the `width`-byte sequence at `at`.
    def _scalar_at(self, at: Int, width: Int) -> Int:
        var lead = Int(self.data[at])
        if width == 1:
            return lead
        var value = lead % 32 if width == 2 else (lead % 16 if width == 3 else lead % 8)
        var i = 1
        while i < width:
            value = value * 64 + Int(self.data[at + i]) % 64
            i += 1
        return value

    # Non-raising UTF-8 lead-byte width (a stray continuation byte counts as
    # one so scans always advance).
    def _lead_width(self, lead: Int) -> Int:
        if lead < 224:
            return 1 if lead < 192 else 2
        return 3 if lead < 240 else 4

    def _is_continuation(self, b: Int) -> Bool:
        return b >= 128 and b < 192

    def _is_posix_space_byte(self, b: Int) -> Bool:
        return b == 32 or (b >= 9 and b <= 13) or (b >= 28 and b <= 30)

    # The byte width of the Python-space codepoint at byte offset `at`, or 0.
    def _space_width_at(self, at: Int) -> Int:
        var b = Int(self.data[at])
        if self._is_posix_space_byte(b):
            return 1
        return self._unicode_separator_width_at(at)

    # The byte width of the line boundary at byte offset `at` (`\r\n` counts
    # as one), or 0.
    def _newline_width_at(self, at: Int) -> Int:
        var b = Int(self.data[at])
        if b == 13:
            if at + 1 < self.size and Int(self.data[at + 1]) == 10:
                return 2
            return 1
        if (b >= 9 and b <= 13) or (b >= 28 and b <= 30):
            return 1
        return self._unicode_separator_width_at(at)

    # U+0085 (C2 85), U+2028 (E2 80 A8), and U+2029 (E2 80 A9).
    def _unicode_separator_width_at(self, at: Int) -> Int:
        var b = Int(self.data[at])
        if b == 0xC2 and at + 1 < self.size and Int(self.data[at + 1]) == 0x85:
            return 2
        if b == 0xE2 and at + 2 < self.size and Int(self.data[at + 1]) == 0x80:
            var b2 = Int(self.data[at + 2])
            if b2 == 0xA8 or b2 == 0xA9:
                return 3
        return 0

    def _lstrip_bound(self, start: Int, end: Int) -> Int:
        var at = start
        while at < end and self._is_posix_space_byte(Int(self.data[at])):
            at += 1
        return at

    def _rstrip_bound(self, start: Int, end: Int) -> Int:
        var at = end
        while at > start and self._is_posix_space_byte(Int(self.data[at - 1])):
            at -= 1
        return at

    def _lstrip_chars_bound(self, chars: String, start: Int, end: Int) -> Int:
        var at = start
        while at < end:
            var width = self._lead_width(Int(self.data[at]))
            if not chars._has_sequence(self, at, width):
                break
            at += width
        return at

    def _rstrip_chars_bound(self, chars: String, start: Int, end: Int) -> Int:
        var at = end
        while at > start:
            var head = at - 1
            while head > start and self._is_continuation(Int(self.data[head])):
                head -= 1
            if not chars._has_sequence(self, head, at - head):
                break
            at = head
        return at

    # Whether the `width` bytes of `other` at `at` occur in this buffer.  In
    # valid UTF-8 a whole sequence matches only at a codepoint boundary, so
    # this is codepoint membership.
    def _has_sequence(self, other: String, at: Int, width: Int) -> Bool:
        var pos = 0
        while pos + width <= self.size:
            var i = 0
            while i < width and Int(self.data[pos + i]) == Int(other.data[at + i]):
                i += 1
            if i == width:
                return True
            pos += 1
        return False

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher.update(StringSpan(self))

    def __getitem__(self, *, byte: Int) raises -> Byte:
        if byte < 0:
            raise Error("String byte index out of range")
        if byte >= self.size:
            raise Error("String byte index out of range")
        return self.data[byte]

    def __getitem__(self, *, codepoint: Int) raises -> Codepoint:
        if codepoint < 0:
            raise Error("String codepoint index out of range")
        var index = 0
        var seen = 0
        while index < self.size:
            var lead = Int(self.data[index])
            var width = self._sequence_width(lead)
            var value = self._decode_at(index, width)
            if seen == codepoint:
                var text = self._with_bytes(index, width)
                return Codepoint(value, text: text^)
            seen += 1
            index += width
        raise Error("String codepoint index out of range")

    def count_codepoints(self) raises -> Int:
        var index = 0
        var count = 0
        while index < self.size:
            var lead = Int(self.data[index])
            index += self._sequence_width(lead)
            count += 1
        if index != self.size:
            raise Error("String buffer ends inside a UTF-8 sequence")
        return count

    def __getitem__(self, *, grapheme: Int) raises -> Self:
        if grapheme < 0:
            raise Error("String grapheme index out of range")
        var index = 0
        var seen = 0
        while index < self.size:
            var end = self._next_grapheme_end(index)
            if seen == grapheme:
                return self._with_bytes(index, end - index)
            seen += 1
            index = end
        raise Error("String grapheme index out of range")

    def count_graphemes(self) raises -> Int:
        var index = 0
        var count = 0
        while index < self.size:
            index = self._next_grapheme_end(index)
            count += 1
        return count

    # Ordinary String iteration yields borrowed grapheme-cluster StringSpan
    # views (current Mojo).
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return _GraphemeIter(StringSpan(self), 0)

    # UTF-8 leading-byte arithmetic: the sequence width a lead byte declares.
    def _sequence_width(self, lead: Int) raises -> Int:
        if lead < 128:
            return 1
        if lead < 192:
            raise Error("String buffer is not valid UTF-8")
        if lead < 224:
            return 2
        if lead < 240:
            return 3
        if lead < 248:
            return 4
        raise Error("String buffer is not valid UTF-8")

    # Decode the scalar value of the `width`-byte sequence at `start`.
    def _decode_at(self, start: Int, width: Int) raises -> Int:
        if start + width > self.size:
            raise Error("String buffer ends inside a UTF-8 sequence")
        var lead = Int(self.data[start])
        var value = lead
        if width == 2:
            value = lead - 192
        elif width == 3:
            value = lead - 224
        elif width == 4:
            value = lead - 240
        var i = 1
        while i < width:
            var continuation = Int(self.data[start + i])
            if continuation < 128:
                raise Error("String buffer is not valid UTF-8")
            if continuation >= 192:
                raise Error("String buffer is not valid UTF-8")
            value = value * 64 + (continuation - 128)
            i += 1
        return value

    # The byte offset one past the extended grapheme cluster starting at
    # `start`: decode the first codepoint, then extend while the pair rules
    # join, tracking the run of consecutive regional indicators (class 7).
    def _next_grapheme_end(self, start: Int) raises -> Int:
        var index = start
        var lead = Int(self.data[index])
        var width = self._sequence_width(lead)
        var prev_class = self._grapheme_class(self._decode_at(index, width))
        index += width
        var ri_run = 0
        if prev_class == 7:
            ri_run = 1
        while index < self.size:
            lead = Int(self.data[index])
            width = self._sequence_width(lead)
            var next_class = self._grapheme_class(self._decode_at(index, width))
            if not self._grapheme_joins(prev_class, next_class, ri_run):
                return index
            if next_class == 7:
                ri_run += 1
            else:
                ri_run = 0
            prev_class = next_class
            index += width
        return index

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
        var start_byte = 0
        var end_byte = 0
        try:
            var total = self.count_codepoints()
            var start = codepoint.start.or_else(0)
            var end = codepoint.end.or_else(total)
            check_slice_bounds(start, end, total)
            start_byte = self._codepoint_offset(start)
            end_byte = self._codepoint_offset(end)
        except e:
            _mojito_abort("String buffer is not valid UTF-8")
        var view = StringSpan(self)
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

    # The byte offset after `count` codepoints (strict: `count` must not
    # exceed the codepoint count).
    def _codepoint_offset(self, count: Int) raises -> Int:
        var index = 0
        var seen = 0
        while seen < count:
            if index >= self.size:
                _mojito_abort("String codepoint slice bounds out of range")
            var lead = Int(self.data[index])
            index += self._sequence_width(lead)
            seen += 1
        return index

    # The byte offset after `count` extended grapheme clusters (strict).
    def _grapheme_offset(self, count: Int) raises -> Int:
        var index = 0
        var seen = 0
        while seen < count:
            if index >= self.size:
                _mojito_abort("String grapheme slice bounds out of range")
            index = self._next_grapheme_end(index)
            seen += 1
        return index

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
                    out._append_bytes_of(String("\\\\"), 0, 2)
                elif b == 39:
                    out._append_bytes_of(String("\\'"), 0, 2)
                elif b == 10:
                    out._append_bytes_of(String("\\n"), 0, 2)
                elif b == 9:
                    out._append_bytes_of(String("\\t"), 0, 2)
                else:
                    out._append_bytes_of(String("\\r"), 0, 2)
                run_start = i + 1
            i += 1
        out._append_bytes_of(self, run_start, self.size - run_start)
        out._append_bytes_of(String("'"), 0, 1)
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
# not offer — return sub-views of the same buffer. Codepoint- and
# grapheme-level operations delegate through an eager `to_string()` copy
# for decoding while the returned views stay borrowed from this buffer.
struct StringSpan[mut: Bool, //, origin: Origin[mut=mut]](
    Boolable, Equatable, Hashable, ImplicitlyCopyable, Iterable, Movable, Writable
):
    comptime Element = StringSpan[Self.origin]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _GraphemeIter[iterable_origin]

    var _data: Pointer[Byte, Self.origin._get_owned_interior["bytes"]]
    var _size: Int

    def __init__(out self, ref [Self.origin] src: String):
        self._data = src.data.unsafe_origin_cast[
            origin._get_owned_interior["bytes"]
        ]()
        self._size = src.size

    # Ordinary StringSpan iteration also yields grapheme-cluster sub-views.
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return _GraphemeIter(self, 0)

    def __len__(self) -> Int:
        return self._size

    def byte_length(self) -> Int:
        return self._size

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

    def __contains__(self, substr: String) -> Bool:
        return self.to_string()._find_from(substr, 0) >= 0

    # The strip family and affix removal return sub-views of this buffer
    # (offsets computed on an eager copy of the bytes).
    def strip(self) -> Self:
        var text = self.to_string()
        var start = text._lstrip_bound(0, text.size)
        return self._sub_view(start, text._rstrip_bound(start, text.size))

    def strip(self, chars: String) -> Self:
        var text = self.to_string()
        var start = text._lstrip_chars_bound(chars, 0, text.size)
        return self._sub_view(start, text._rstrip_chars_bound(chars, start, text.size))

    def lstrip(self) -> Self:
        var text = self.to_string()
        return self._sub_view(text._lstrip_bound(0, text.size), self._size)

    def lstrip(self, chars: String) -> Self:
        var text = self.to_string()
        return self._sub_view(text._lstrip_chars_bound(chars, 0, text.size), self._size)

    def rstrip(self) -> Self:
        var text = self.to_string()
        return self._sub_view(0, text._rstrip_bound(0, text.size))

    def rstrip(self, chars: String) -> Self:
        var text = self.to_string()
        return self._sub_view(0, text._rstrip_chars_bound(chars, 0, text.size))

    def removeprefix(self, prefix: String, /) -> Self:
        if self.to_string().startswith(prefix):
            return self._sub_view(prefix.size, self._size)
        return self

    def removesuffix(self, suffix: String, /) -> Self:
        if suffix.size > 0 and self.to_string().endswith(suffix):
            return self._sub_view(0, self._size - suffix.size)
        return self

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_bytes(self.as_bytes())

    def as_bytes(self) -> Span[Byte, Self.origin]:
        return Span[Byte](
            unsafe_ptr=self._data.unsafe_origin_cast[MutUntrackedOrigin](), length=self._size
        )

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
        var text = self.to_string()
        return text[codepoint=codepoint]

    def __getitem__(self, *, grapheme: Int) raises -> String:
        var text = self.to_string()
        return text[grapheme=grapheme]

    def count_codepoints(self) raises -> Int:
        return self.to_string().count_codepoints()

    def count_graphemes(self) raises -> Int:
        return self.to_string().count_graphemes()

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
        var start_byte = 0
        var end_byte = 0
        try:
            var text = self.to_string()
            var total = text.count_codepoints()
            var start = codepoint.start.or_else(0)
            var end = codepoint.end.or_else(total)
            check_slice_bounds(start, end, total)
            start_byte = text._codepoint_offset(start)
            end_byte = text._codepoint_offset(end)
        except e:
            _mojito_abort("StringSpan buffer is not valid UTF-8")
        return self._sub_view(start_byte, end_byte)

    def __getitem__(self, *, grapheme: ContiguousSlice) -> Self:
        var start_byte = 0
        var end_byte = 0
        try:
            var text = self.to_string()
            var total = text.count_graphemes()
            var start = grapheme.start.or_else(0)
            var end = grapheme.end.or_else(total)
            check_slice_bounds(start, end, total)
            start_byte = text._grapheme_offset(start)
            end_byte = text._grapheme_offset(end)
        except e:
            _mojito_abort("StringSpan buffer is not valid UTF-8")
        return self._sub_view(start_byte, end_byte)

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

    # An iterator is its own iterable (`for x in s.codepoints()`).
    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.index >= len(self.src):
            raise StopIteration()
        var start = self.index
        var end = start
        try:
            var text = self.src.to_string()
            end = text._next_grapheme_end(start)
        except e:
            _mojito_abort("String buffer is not valid UTF-8")
        self.index = end
        return self.src._sub_view(start, end)

    # Remaining grapheme clusters (`Sized`, as upstream's iterator).
    def __len__(self) -> Int:
        var count = 0
        var at = self.index
        try:
            var text = self.src.to_string()
            while at < text.size:
                at = text._next_grapheme_end(at)
                count += 1
        except e:
            _mojito_abort("String buffer is not valid UTF-8")
        return count


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
        if self.index >= len(self.src):
            raise StopIteration()
        var text = self.src.to_string()
        var width = text._lead_width(Int(text.data[self.index]))
        var scalar = text._scalar_at(self.index, width)
        var piece = text._with_bytes(self.index, width)
        self.index += width
        return Codepoint(scalar, text: piece^)

    def __len__(self) -> Int:
        var text = self.src.to_string()
        var count = 0
        var at = self.index
        while at < text.size:
            at += text._lead_width(Int(text.data[at]))
            count += 1
        return count


# `String.codepoint_slices()`: one-codepoint sub-views of the source buffer.
@fieldwise_init
struct _CodepointSliceIter[
    iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
](Copyable, ImplicitlyCopyable, Iterator, Movable):
    comptime Element = StringSpan[Self.iterable_origin]

    var src: StringSpan[Self.iterable_origin]
    var index: Int

    # An iterator is its own iterable (`for x in s.codepoints()`).
    def __iter__(self) -> Self:
        return self

    def __next__(mut self) raises StopIteration -> StringSpan[Self.iterable_origin]:
        if self.index >= len(self.src):
            raise StopIteration()
        var text = self.src.to_string()
        var start = self.index
        var end = start + text._lead_width(Int(text.data[start]))
        self.index = end
        return self.src._sub_view(start, end)

    def __len__(self) -> Int:
        var text = self.src.to_string()
        var count = 0
        var at = self.index
        while at < text.size:
            at += text._lead_width(Int(text.data[at]))
            count += 1
        return count
