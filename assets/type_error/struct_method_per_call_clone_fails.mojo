# expect: function instantiation of `S.f
# A struct's generic method is instantiated per call; the instance a reached
# call needs fails, as at the pin, rather than trapping at run time.
struct S:
    def __init__(out self):
        pass

    def f[k: Int](self):
        comptime for i in range(k):
            comptime q = [1, 2][i]
            print(q)


def main():
    S().f[2]()
    S().f[5]()
