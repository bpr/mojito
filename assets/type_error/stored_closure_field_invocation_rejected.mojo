# expect: capturing
# A capturing closure no longer erases its environment into a plain
# `def(...)` field: the store itself rejects with the environment shown.
# (A `capturing[_]`-annotated field accepts it instead.)
@fieldwise_init
struct Holder:
    var callback: def() -> Int

def main():
    var values = [1, 2]
    for ref x in values:
        def peek() unified {ref x} -> Int:
            return x
        var holder = Holder(peek)
