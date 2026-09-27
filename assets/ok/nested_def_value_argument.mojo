# A nested generic `def` builds a callee's value argument from its own
# parameter (`scaled[k + 1]()` inside `inner[k: Int]`). Each call binding
# the nested `def`'s parameter specializes its body.
def scaled[n: Int]() -> Int:
    return n


def outer[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        return scaled[k + 1]() + k

    return inner[10]() + inner[20]()


def main():
    print(outer[1]())
