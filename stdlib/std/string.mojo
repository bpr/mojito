# A self-hosted UTF-8 String.  `data` owns `size` initialized bytes in a
# `cap`-byte allocation.  Construction from a literal is the compiler's
# literal-to-struct bridge (the byte buffer is filled from the literal's
# UTF-8 bytes at the call); every other operation is ordinary library code
# over the byte buffer.  Slicing and the result APIs (`find`/`rfind`/
# `startswith`/`endswith`/`split`) work in byte offsets, like `len`.
#
# `s[codepoint=i]` yields a `Codepoint` value carrying both the decoded
# scalar and the character's text.  `s[grapheme=i]` and `grapheme_count()`
# segment extended grapheme clusters with a documented UAX #29 subset: a
# hand-maintained essentials classifier plus arithmetic Hangul, with GB11
# simplified to "never break after ZWJ" and GB9b (Prepend) omitted.

from std.memory import unsafe_alloc

from std.collections.list import List
from std.optional import Optional

struct String(Comparable, Copyable, Equatable, Hashable, Movable, Writable):
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

    def __contains__(self, sub: Self) -> Bool:
        return self._find_from(sub, 0) >= 0

    # Result APIs use byte offsets, like `len` and the byte-wise slice.  An
    # empty needle matches everywhere (Python semantics): `find` reports the
    # search start, `rfind` the end, the affix tests True.

    def find(self, substr: Self) -> Int:
        return self._find_from(substr, 0)

    def rfind(self, substr: Self) -> Int:
        var start = self.size - substr.size
        while start >= 0:
            if self._matches_at(substr, start):
                return start
            start -= 1
        return -1

    def startswith(self, prefix: Self) -> Bool:
        if prefix.size > self.size:
            return False
        return self._matches_at(prefix, 0)

    def endswith(self, suffix: Self) -> Bool:
        if suffix.size > self.size:
            return False
        return self._matches_at(suffix, self.size - suffix.size)

    # Eager owned pieces rather than current Mojo's borrowed StringSlice
    # views (the recorded eager-result divergence).  Splitting on the empty
    # separator raises, as in Mojo and Python.
    def split(self, sep: Self) raises -> List[String]:
        if sep.size == 0:
            raise Error("String split separator is empty")
        var parts = List[String]()
        var start = 0
        var at = self._find_from(sep, start)
        while at >= 0:
            parts.append(self[start:at])
            start = at + sep.size
            at = self._find_from(sep, start)
        parts.append(self[start:self.size])
        return parts^

    # Naive forward byte search from byte offset `start`: the offset of the
    # first match at or after it, or -1.
    def _find_from(self, sub: Self, start: Int) -> Int:
        var at = start
        while at + sub.size <= self.size:
            if self._matches_at(sub, at):
                return at
            at += 1
        return -1

    # Whether `sub`'s bytes appear verbatim at byte offset `at`; the caller
    # keeps `at + sub.size` within the buffer.
    def _matches_at(self, sub: Self, at: Int) -> Bool:
        var i = 0
        while i < sub.size:
            if Int(self.data[at + i]) != Int(sub.data[i]):
                return False
            i += 1
        return True

    def __hash__(self) -> UInt:
        # DJB2 over the UTF-8 bytes — the bundled IncrementalHasher recipe.
        var state = UInt(5381)
        var i = 0
        while i < self.size:
            state = state * UInt(33) + UInt(Int(self.data[i]))
            i += 1
        return state

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
                return Codepoint(value, text: text._as_string_literal())
            seen += 1
            index += width
        raise Error("String codepoint index out of range")

    def codepoint_count(self) raises -> Int:
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

    def grapheme_count(self) raises -> Int:
        var index = 0
        var count = 0
        while index < self.size:
            index = self._next_grapheme_end(index)
            count += 1
        return count

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

    # Byte-wise, non-raising, like the builtin literal slice: `indices()`
    # normalizes Python-style bounds (clamping, negative wrap, stride,
    # reversal).  A cut inside a multibyte sequence keeps the raw bytes;
    # codepoint/grapheme operations still validate lazily, and the literal
    # read-back renders invalid sequences lossily.
    def __getitem__(self, slice: Slice) -> Self:
        var bounds = slice.indices(self.size)
        var start = bounds[0]
        var stop = bounds[1]
        var step = bounds[2]
        var count = 0
        var probe = start
        if step > 0:
            while probe < stop:
                count += 1
                probe += step
        else:
            while probe > stop:
                count += 1
                probe += step
        var result = String("")
        result.data.unsafe_free()
        result.data = unsafe_alloc[Byte](count)
        result.size = count
        result.cap = count
        var source = start
        var filled = 0
        while filled < count:
            result.data[filled] = self.data[source]
            source += step
            filled += 1
        return result^

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

    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("\"")
        writer.write(self._as_string_literal())
        writer.write("\"")

# A decoded Unicode scalar together with its character text.  Produced by
# `String.__getitem__(*, codepoint=...)`, which owns the bytes and derives
# `text` through the struct-to-literal bridge, or by the public
# `Codepoint.from_u32(scalar)` (Mojito is Int-based), which UTF-8-encodes
# the scalar in ordinary library code through runtime `Byte(Int)`
# conversions.
struct Codepoint(
    Comparable, Copyable, Equatable, Deinitable, Intable, Movable, Writable
):
    var _scalar: Int
    var _text: StringLiteral

    def __init__(out self, scalar: Int, *, text: StringLiteral):
        self._scalar = scalar
        self._text = text

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
        return Optional[Codepoint](
            Codepoint(scalar, text: text._as_string_literal()), True
        )

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
