# A raise in a `finally` copied before a `continue` in the protected body
# names the enclosing `try`, as `lit.try.raise` names its label, so it
# skips the handler of the `try` whose body now holds the copy.
#
# Expected output:
#   fin 0
#   outer caught in finally
#   body 1
#   fin 1
#   end


def f(x: Int) raises -> Int:
    if x > 0:
        raise Error("in finally")
    return x


def main():
    for i in range(2):
        try:
            try:
                if i == 0:
                    continue
                print("body", i)
            except e:
                print("inner caught", e)
            finally:
                print("fin", i)
                _ = f(1 - i)
        except e:
            print("outer caught", e)
    print("end")
