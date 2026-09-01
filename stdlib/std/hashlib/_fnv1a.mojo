struct Fnv1a(Defaultable, Hasher):
    var _value: UInt64

    def __init__(out self):
        self._value = UInt64(0xCBF29CE484222325)

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        var i = 0
        while i < len(data):
            self._value ^= data[i].cast[DType.uint64]()
            self._value *= UInt64(0x100000001B3)
            i += 1

    def _update_with_simd(mut self, value: UInt64):
        self._value ^= value
        self._value *= UInt64(0x100000001B3)

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        return self._value
