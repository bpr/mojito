# A user `Indexer` spells upstream's requirement, `__mlir_index__(self) ->
# __mlir_type.index` (represented as `Int` by the VM), and normalizes a
# subscript through it (pinned Mojo a79fbdf59f2: prints 7).
@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var values = [3, 7]
    print(values[Offset(1)])
