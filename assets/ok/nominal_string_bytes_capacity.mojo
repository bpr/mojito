# Byte-level access and capacity: `as_bytes` is a borrowed Span over the
# buffer (a pointer-backed Span), `unsafe_ptr` the raw interior pointer,
# `capacity_bytes` the allocation size, `resize` grows with a fill byte or
# shrinks to a codepoint boundary, and `append` adds one codepoint.
def main() raises:
    var s = String("héllo")
    print(s.capacity_bytes() >= s.byte_length())
    s.resize(3)
    print(s, s.byte_length())
    s.resize(5, 65)
    print(s, s.byte_length())
    var maybe = Codepoint.from_u32(0xE9)
    s.append(maybe.value())
    print(s, s.byte_length())
    var empty = String("")
    empty.resize(2, 120)
    print(empty, empty.capacity_bytes() >= 2)
    var hb = String("hé")
    var bytes = hb.as_bytes()
    print(len(bytes), bytes[0], bytes[1], bytes[2])
    var view = StringSpan(hb)
    print(hash(view) == hash(view), hash(view) == hash(hb), len(view.as_bytes()))
