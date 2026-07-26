struct Wrapped:
    var value: Int

    @implicit
    def __init__(out self, value: Int):
        print("convert", value)
        self.value = value

@fieldwise_init
struct Box:
    var value: Int

    def __getitem__(self, index: Wrapped) -> Int:
        print("get", index.value)
        return self.value

    def __setitem__(mut self, index: Int, value: Int):
        print("set", index, value)
        self.value = value

def next_index() -> Int:
    print("index")
    return 0

def rhs() -> Int:
    print("rhs")
    return 2

def main():
    var box = Box(40)
    box[next_index()] += rhs()
    print(box.value)
