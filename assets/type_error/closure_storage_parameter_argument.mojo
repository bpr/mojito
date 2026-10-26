# A capture-list closure's value is its storage struct, a run-time value, so
# it cannot bind a compile-time callable parameter.
# expect: cannot use a dynamic value in a parameter list
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
