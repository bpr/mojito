@fieldwise_init
struct StopIteration:
    pass


struct Item(Deinitable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


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
struct Values(Deinitable, Movable):
    var value: Int

    def __iter__(ref self) -> ValueIter:
        return ValueIter(self.value, False)

    def __iter__(var self) -> ValueIter:
        return ValueIter(self.value, False)


def take(var item: Item):
    print("take", item.value)


def main():
    var imm_borrowed = Values(1)
    for item in imm_borrowed:
        print("imm borrowed", item.value)

    var var_borrowed = Values(2)
    for var item in var_borrowed:
        take(item^)

    var ref_borrowed = Values(3)
    for ref item in ref_borrowed:
        item.value += 10
        print("ref borrowed", item.value)

    var imm_consumed = Values(4)
    for item in imm_consumed^:
        print("imm consumed", item.value)

    var var_consumed = Values(5)
    for var item in var_consumed^:
        take(item^)

    var ref_consumed = Values(6)
    for ref item in ref_consumed^:
        item.value += 10
        print("ref consumed", item.value)
