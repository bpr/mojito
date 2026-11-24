# A member read in an annotation types its base as an expression, so a
# local `comptime` construction hides a module struct of its name: `w.E`
# reads `W[3]`'s `E`, not the module `w`'s.
struct W[n: Int]:
    comptime E = Bool

    def __init__(out self):
        pass


struct w:
    comptime E = Int


def main():
    comptime w = W[3]()
    var a: w.E = True
    print(a)
