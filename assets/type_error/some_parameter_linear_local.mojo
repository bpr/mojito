# A `Some[Trait]` parameter's binder is an archetype of its declaration, as
# `[T: Writable & Copyable, //]` is: a local copy of it is abandoned unless
# its bound proves `Deinitable`.
# expect: 'u' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'Writable'
def show(t: Some[Writable & Copyable]):
    var u = t.copy()
    print(u)


def main():
    show(1)
