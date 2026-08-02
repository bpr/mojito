trait ReferenceIteratorContract:
    def __next__(mut self, ref source: Int) -> ref[source] Int:
        ...

@fieldwise_init
struct ValueIterator(ReferenceIteratorContract):
    var value: Int

    def __next__(mut self, ref source: Int) -> Int:
        return source

def main():
    pass
