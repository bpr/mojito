# A tuple element's memory-only temporary is destroyed in storage when a
# later element's call raises before the tuple is built.
def may(x: Int) raises -> Int:
    if x > 0:
        raise Error("boom")
    return x


def first(s: String, n: Int) raises -> Int:
    var t = (s + "!", may(n))
    return t[1]


def main():
    try:
        print(first(String("ab"), 0))
        print(first(String("ab"), 1))
    except e:
        print("caught", e)
