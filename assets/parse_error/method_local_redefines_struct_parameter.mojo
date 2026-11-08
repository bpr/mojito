# expect: invalid redefinition of 'n'
# A method body's top level shares its parameter scope, which holds the
# struct's parameters, so a local there may not reuse one's name. A nested
# block may.
struct H[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        if True:
            var n = 1
            print(n)
        var n = 2
        print(n)

def main():
    H[1]().f()
