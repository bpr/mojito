def f[T: ImplicitlyCopyable & Writable & Deinitable, p: Tuple[Int, Int]](x: T):
    print(x, p[0])


def main():
    f[Int, (1, 2)](3)
    f[String, (4, 5)]("s")
