# A `Some[Trait]` parameter's binder is an infer-only binder of its own
# declaration, so a nested `def` taking a `Some[Writable]` of its own binds
# it apart from its enclosing declaration's: `inner(7)` deduces `Int` while
# `outer("o")` deduces a string, and two declarations' first `Some`
# parameters are two binders.
def outer(x: Some[Writable]):
    def inner(y: Some[Writable]):
        print(y)

    inner(7)
    print(x)


def first(a: Some[Writable]) -> String:
    return String(a)


def second(b: Some[Writable]) -> String:
    return String(b) + "!"


def main():
    outer("o")
    print(first(1), second(2.5), first("z"), second(True))
