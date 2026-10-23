# A `return` or `break` from a `for` loop's `try` runs the copied
# `finally` while the loop still owns the element it reads: the copy goes
# before the loop iterator's keep-alive.
#
# Expected output:
#   finally a
#   finally b
#   1
#   fin p
#   leaving
#   fin q
#   end


def f(xs: List[String]) -> Int:
    for x in xs:
        try:
            if x == "b":
                return 1
        finally:
            print("finally", x)
    return 0


def main():
    var xs = List[String]()
    xs.append("a")
    xs.append("b")
    xs.append("c")
    print(f(xs))
    var owned = List[String]()
    owned.append("p")
    owned.append("q")
    for y in owned^:
        try:
            if y == "q":
                print("leaving")
                break
        finally:
            print("fin", y)
    print("end")
