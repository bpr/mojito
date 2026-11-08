# An initializer read as a value with no expected type is an overloaded
# reference: the struct's synthesized move initializer is always another
# candidate.
# expect: requires a contextual callable type
struct Q:
    var a: Int

    def __init__(out self, k: Int):
        self.a = k


def main():
    var g = Q.__init__
    print(g(3).a)
