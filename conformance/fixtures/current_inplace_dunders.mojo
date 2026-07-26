@fieldwise_init
struct Counter:
    var value: Int

    def __iadd__(mut self, amount: Int):
        self.value += amount

def main():
    var counter = Counter(40)
    counter += 2
    print(counter.value)
