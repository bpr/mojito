@fieldwise_init
struct StopIteration:
    pass


struct Item(Deinitable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value

    def __deinit__(deinit self):
        print("drop", self.value)


@fieldwise_init
struct ValueIter:
    var value: Int
    var done: Bool

    def __next__(mut self) raises StopIteration -> Item:
        if self.done:
            raise StopIteration()
        self.done = True
        return Item(self.value)


@fieldwise_init
struct Values:
    var value: Int

    def __iter__(ref self) -> ValueIter:
        return ValueIter(self.value, False)


def main():
    var imm_source = Values(1)
    for item in imm_source:
        print("imm", item.value)

    var var_source = Values(2)
    for var item in var_source:
        print("var", item.value)

    var ref_source = Values(3)
    for ref item in ref_source:
        item.value += 10
        print("ref", item.value)

    print("done")
