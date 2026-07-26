@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> Int:
        return self.value


def main():
    var values = [3, 7]
    print(values[Offset(1)])
