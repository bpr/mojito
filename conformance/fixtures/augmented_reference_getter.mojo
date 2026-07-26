@fieldwise_init
struct Box:
    var value: Int

    def __getitem__(mut self, index: Int) -> ref[origin_of(self.value)] Int:
        print("get", index)
        return self.value

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
