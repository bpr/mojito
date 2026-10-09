# A closure with a by-value capture (`{var x}`) bound to a compile-time
# callable parameter. The pin rejects the binding ("cannot use a dynamic value
# in a parameter list"); Mojito accepts it and prints 10 (roadmap R534). A
# generic closure of the same shape is refused only by the elaborator
# ("captures by value").
#
# Observed 2026-10-09 against `Mojo 1.2.0.dev2026092105`.
def invoke[
    origins: OriginSet,
    //,
    callback: def() capturing[origins] -> Int,
]() -> Int:
    return callback()


def main():
    var x = 5

    def inner() {var x} -> Int:
        return x * 2

    x = 100
    print(invoke[inner]())
