@fieldwise_init
struct Item(Copyable, Movable):
    var value: Int

def bump(mut value: Int):
    value += 1

def observe(ref value: Int):
    print(value)

def main() raises:
    var values = {"a": Item(10)}
    ref field = values["a"].value
    print(field)
    bump(values["a"].value)
    observe(values["a"].value)
    print(values["a"].value)
