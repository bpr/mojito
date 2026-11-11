# expect: attempt to resolve a recursive reference to declaration 'W.a'
# `a`'s signature reads `b`, whose signature reads `a`: neither resolves.


struct W:
    def __init__(out self):
        pass

    def a(self) -> type_of(self.b()):
        return self.b()

    def b(self) -> type_of(self.a()):
        return self.a()


def main():
    print(W().a())
