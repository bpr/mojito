@fieldwise_init
struct PlaceIndex(Copyable, Movable):
    var value: Int

    def __getitem__(mut self, mut index: Int) -> Int:
        self.value += 1
        index += 1
        return index


def main():
    var value = PlaceIndex(40)
    print(value[value.value])
