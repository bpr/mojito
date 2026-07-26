@fieldwise_init
struct RefBox[origin: Origin[mut=True]]:
    var value: ref[origin] Int


@fieldwise_init
struct RefList[origin: Origin[mut=True]]:
    var values: List[ref[origin] Int]


@fieldwise_init
struct Item(Copyable, Movable):
    var value: Int

    def bump(mut self):
        self.value += 2


@fieldwise_init
struct RefItemList[origin: Origin[mut=True]]:
    var values: List[ref[origin] Item]


def main():
    var value = 40
    ref alias = value
    var box = RefBox(alias)
    box.value += 2
    print(value)

    var nested = 40
    ref nested_alias = nested
    var refs = RefList([nested_alias])
    refs.values[0] += 2
    print(nested)

    var item = Item(40)
    ref item_alias = item
    var items = RefItemList([item_alias])
    items.values[0].bump()
    print(item.value)
