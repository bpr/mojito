# expect: unqualified access to struct parameter 'n'; use 'Self.n' instead
# A method's `where` clause reads its struct's value parameter as `Self.n`.
struct W[n: Int]:
    def __init__(out self):
        pass

    def f(self) -> Int where n > 0:
        return 1


def main():
    var w = W[2]()
    print(w.f())
