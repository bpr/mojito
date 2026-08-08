# A self-hosted UTF-8 String.  `data` owns `size` initialized bytes in a
# `cap`-byte allocation.  Construction from a literal is the compiler's
# literal-to-struct bridge (the byte buffer is filled from the literal's
# UTF-8 bytes at the call); every other operation is ordinary library code
# over the byte buffer.  The compile-time literal type (`StringLiteral`)
# and its builtin operations are unchanged; `String(...)` with a
# non-literal argument keeps the builtin Writable stringification until
# the type-split migration.

struct String(Comparable, Copyable, Equatable, Hashable, Movable, Writable):
    var data: UnsafePointer[Byte]
    var size: Int
    var cap: Int

    def __init__(out self, literal: StringLiteral):
        # The compiler replaces this call: `data`/`size`/`cap` are filled
        # from the literal's UTF-8 bytes.  The body only establishes the
        # field contract and never executes.
        self.size = 0
        self.cap = 1
        self.data = UnsafePointer[Byte].alloc(self.cap)

    def __init__(out self, *, copy: Self):
        self.size = copy.size
        self.cap = copy.cap
        self.data = UnsafePointer[Byte].alloc(self.cap)
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

    def __del__(deinit self):
        self.data.free()

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

    def __getitem__(self, *, codepoint: Int) raises -> Int:
        if codepoint < 0:
            raise Error("String codepoint index out of range")
        var index = 0
        var seen = 0
        while index < self.size:
            var lead = Int(self.data[index])
            var width = self._sequence_width(lead)
            var value = self._decode_at(index, width)
            if seen == codepoint:
                return value
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

    def __getitem__(self, slice: Slice) raises -> Self:
        var bounds = slice.indices(self.size)
        var start = bounds[0]
        var stop = bounds[1]
        var step = bounds[2]
        if step != 1:
            raise Error("String slicing is contiguous; a stride is not supported")
        if stop < start:
            stop = start
        if not self._is_boundary(start):
            raise Error("String slice start splits a UTF-8 sequence")
        if not self._is_boundary(stop):
            raise Error("String slice end splits a UTF-8 sequence")
        return self._with_bytes(start, stop - start)

    # Whether `index` lands between UTF-8 sequences (never inside one): the
    # buffer edges, or any non-continuation byte.
    def _is_boundary(self, index: Int) -> Bool:
        if index <= 0:
            return True
        if index >= self.size:
            return True
        var lead = Int(self.data[index])
        if lead < 128:
            return True
        return lead >= 192

    def _with_bytes(self, start: Int, count: Int) -> Self:
        var result = String("")
        result.data.free()
        result.data = UnsafePointer[Byte].alloc(count)
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
