@fieldwise_init
struct Counter(ImplicitlyCopyable):
    var value: Int

    def set(mut self, value: Int):
        self.value = value

def main():
    var first = Counter(3)
    var second = first
    second.set(9)
    print(first.value, second.value)
