comptime U256 = SIMD[DType.uint64, 4]
comptime U128 = SIMD[DType.uint64, 2]
comptime MULTIPLE = 6364136223846793005
comptime ROT = 23

def _folded_multiply(lhs: UInt64, rhs: UInt64) -> UInt64:
    var mask = UInt64(0xFFFFFFFF)
    var lhs_low = lhs & mask
    var lhs_high = lhs >> UInt64(32)
    var rhs_low = rhs & mask
    var rhs_high = rhs >> UInt64(32)
    var ll = lhs_low * rhs_low
    var lh = lhs_low * rhs_high
    var hl = lhs_high * rhs_low
    var hh = lhs_high * rhs_high
    var mid = (ll >> UInt64(32)) + (lh & mask) + (hl & mask)
    var low = (ll & mask) | (mid << UInt64(32))
    var high = hh + (lh >> UInt64(32)) + (hl >> UInt64(32)) + (mid >> UInt64(32))
    return low ^ high

def _load_le(data: Span[Byte, _], at: Int, count: Int) -> UInt64:
    var result = UInt64(0)
    var i = 0
    while i < count:
        result |= data[at + i].cast[DType.uint64]() << UInt64(i * 8)
        i += 1
    return result

struct AHasher[key: U256](Defaultable, Hasher):
    var buffer: UInt64
    var pad: UInt64
    var extra_keys: U128

    def __init__(out self):
        var pi_key = Self.key ^ U256(
            0x243F6A8885A308D3,
            0x13198A2E03707344,
            0xA4093822299F31D0,
            0x082EFA98EC4E6C89,
        )
        self.buffer = pi_key[0]
        self.pad = pi_key[1]
        self.extra_keys = U128(pi_key[2], pi_key[3])

    def __init__(out self, seed: U256):
        var pi_key = Self.key ^ U256(
            0x243F6A8885A308D3,
            0x13198A2E03707344,
            0xA4093822299F31D0,
            0x082EFA98EC4E6C89,
        ) ^ seed
        self.buffer = pi_key[0]
        self.pad = pi_key[1]
        self.extra_keys = U128(pi_key[2], pi_key[3])

    def _update(mut self, value: UInt64):
        self.buffer = _folded_multiply(value ^ self.buffer, UInt64(MULTIPLE))

    def _large_update(mut self, first: UInt64, second: UInt64):
        var combined = _folded_multiply(
            first ^ self.extra_keys[0], second ^ self.extra_keys[1]
        )
        # `rotate_bits_left[ROT](...)` from `std.bit`, spelled inline so the
        # hasher stays a plain nominal body (a value-parameterized helper
        # cannot cross the compile-time execution boundary).
        var mixed = (self.buffer + self.pad) ^ combined
        self.buffer = (mixed << UInt64(ROT)) | (mixed >> UInt64(64 - ROT))

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        var length = len(data)
        self.buffer = (self.buffer + UInt64(length)) * UInt64(MULTIPLE)
        if length > 16:
            self._large_update(
                _load_le(data, length - 16, 8), _load_le(data, length - 8, 8)
            )
            var at = 0
            while length - at > 16:
                self._large_update(_load_le(data, at, 8), _load_le(data, at + 8, 8))
                at += 16
        elif length > 8:
            self._large_update(_load_le(data, 0, 8), _load_le(data, length - 8, 8))
        elif length >= 4:
            self._large_update(_load_le(data, 0, 4), _load_le(data, length - 4, 4))
        elif length >= 2:
            self._large_update(_load_le(data, 0, 2), _load_le(data, length - 1, 1))
        elif length == 1:
            var value = _load_le(data, 0, 1)
            self._large_update(value, value)
        else:
            self._large_update(UInt64(0), UInt64(0))

    # One round per vector: no Mojito dtype is wider than 64 bits, so
    # upstream's multi-round branch for 128-bit lanes never applies.
    def _update_with_simd(mut self, new_data: SIMD[_, _]):
        var u64 = new_data.to_bits[DType.uint64]()
        if u64.length == 1:
            self._update(u64[0])
        else:
            var i = 0
            while i < u64.length:
                self._large_update(u64[i], u64[i + 1])
                i += 2

    def update(mut self, value: Some[Hashable]):
        value.__hash__(self)

    def finish(var self) -> UInt64:
        var rotation = self.buffer & UInt64(63)
        var folded = _folded_multiply(self.buffer, self.pad)
        return (folded << rotation) | (
            folded >> ((UInt64(64) - rotation) & UInt64(63))
        )

# The zero-keyed hasher (`default_hasher`'s type); spelled once so the seeded
# entry points below name it without an applied-type expression in a generic
# body.
comptime _ZeroKeyAHasher = AHasher[U256(0)]

def hash_seeded_bytes(data: ImmPointer[UInt8, _], n: Int, seed: U256) -> UInt64:
    var hasher = _ZeroKeyAHasher(seed)
    hasher._update_with_bytes(
        Span[Byte](unsafe_ptr=data.unsafe_origin_cast[ImmUntrackedOrigin](), length=n)
    )
    return hasher^.finish()

def hash_seeded[T: Hashable](value: T, seed: U256) -> UInt64:
    var hasher = _ZeroKeyAHasher(seed)
    hasher.update(value)
    return hasher^.finish()
