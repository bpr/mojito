@fieldwise_init
struct Item(Copyable, Movable):
    var value: Int


def main() raises:
    var values = {"a": Item(10)}
    ref field = values["a"].value
    print(values["a"].value)
    print(field)
