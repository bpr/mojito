@fieldwise_init
struct Box:
    var value: Int
    var seen: Int

    def __getitem__(self, mut index: Int) -> Int:
        index += 1
        return self.value

    def __setitem__(mut self, index: Int, value: Int):
        self.seen = index
        self.value = value

def main():
    var box = Box(40, -1)
    var index = 0
    box[index] += 2
    print(index, box.seen)
