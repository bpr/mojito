# expect: declares conformance to trait 'Indexer' but is missing method '__mlir_index__'
@fieldwise_init
struct BadIndex(Indexer):
    var value: Int

def main():
    var values = [1, 2]
    print(values[BadIndex(0)])
