# A `comptime for` its template does not serve keys a clone of its `def`
# wherever it sits: inside the body of a served loop or an arm of a kept
# `comptime if`, as at the top of the body.
def twice(n: Int) -> Int:
    return n * 2


def in_loop[n: Int]():
    comptime for i in range(2):
        comptime for x in [twice(n), i]:
            print(x)


def in_arm[n: Int]():
    comptime if n > 1:
        comptime for x in [twice(n), n]:
            print(x)
    else:
        print("small")


def main():
    in_loop[2]()
    in_arm[3]()
    in_arm[1]()
