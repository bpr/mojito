@fieldwise_init
struct Counter:
    var value: Int

    def __getitem__(ref self, index: Int) -> Int:
        print("get", index)
        return self.value

    def __setitem__(mut self, index: Int, value: Int):
        print("set", index, value)
        self.value = value

def next_index() -> Int:
    print("index")
    return 0

def rhs() -> Int:
    print("rhs")
    return 5

def main():
    var counter = Counter(10)
    counter[next_index()] += rhs()
    print(counter.value)
