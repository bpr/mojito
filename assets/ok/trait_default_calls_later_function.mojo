# A trait default is its trait's member, checked once every module
# declaration is bound: its body calls a function declared after the trait
# and after the conformer that inherits it, as at the pin.
trait Greeter:
    def name(self) -> String:
        ...

    def greet(self) -> String:
        return "hi " + self.name() + suffix()


struct P(Greeter):
    def __init__(out self):
        pass

    def name(self) -> String:
        return "p"


def suffix() -> String:
    return "!"


def main():
    print(P().greet())
