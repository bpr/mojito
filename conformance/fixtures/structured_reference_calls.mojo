def borrow(ref item: Int) -> ref[item] Int:
    return item


def element(
    ref values: List[Int], index: Int
) -> ref[origin_of(values[index])] Int:
    return values[index]


def mutate(mut value: Int, fail: Bool) raises:
    value += 1
    if fail:
        raise Error("failed")


@fieldwise_init
struct Box:
    var value: Int

    def get(ref self) -> ref[origin_of(self.value)] Int:
        return self.value

    def bump(mut self, fail: Bool) raises:
        self.value += 1
        if fail:
            raise Error("failed")

    def copy_into(self, mut target: Int):
        target = self.value


def main():
    var direct = 40
    try:
        ref borrowed = borrow(item=direct)
        borrowed += 2
    except error:
        print("unexpected direct")
    print(direct)

    var box = Box(40)
    try:
        ref borrowed = box.get()
        borrowed += 2
    except error:
        print("unexpected method")
    print(box.value)

    var copied = 0
    try:
        box.copy_into(target=copied)
    except error:
        print("unexpected keyword method")
    print(copied)

    var values = [10, 20, 30]
    try:
        ref selected = element(values, 1)
        selected += 2
    except error:
        print("unexpected projection")
    print(values[0], values[1], values[2])

    var changed = 20
    try:
        mutate(value=changed, fail=True)
    except error:
        print("free caught")
    print(changed)

    var counter = Box(20)
    try:
        counter.bump(True)
    except error:
        print("method caught")
    print(counter.value)
