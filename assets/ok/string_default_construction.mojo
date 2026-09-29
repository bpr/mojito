# `String` conforms to `Defaultable`: `String()` is the empty string, a
# `T: Defaultable` bound accepts it, and a tuple with a `String` element
# default-constructs.
def make[T: Defaultable & Writable]() -> T:
    return T()


def main():
    var s = make[String]()
    s += "grown"
    print(s, s.byte_length())
    var t = Tuple[String, Int]()
    print(t[0].byte_length(), t[1])
    t[0] += "tail"
    print(t[0])
