def rotate_bits_left[shift: Int](value: UInt64) -> UInt64:
    comptime if shift == 0:
        return value
    return (value << UInt64(shift)) | (value >> UInt64(64 - shift))
