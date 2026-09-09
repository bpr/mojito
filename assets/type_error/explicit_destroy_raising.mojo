# expect: use of uninitialized value 'transaction'
# A consuming named destructor that raises has still consumed its value: the
# `except` arm sees `transaction` uninitialized and cannot fall back to
# another destructor. Both compilers reject (pinned Mojo a79fbdf59f2).
@explicit_destroy("finish the transaction")
struct Transaction(Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def commit(deinit self) raises:
        raise Error("commit failed")

    def rollback(deinit self):
        print("rolled back", self.id)

def main():
    var transaction = Transaction(9)
    try:
        transaction^.commit()
    except:
        transaction^.rollback()
