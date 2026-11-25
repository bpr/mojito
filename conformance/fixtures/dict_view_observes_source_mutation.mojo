# A stored `keys()` view keeps its dictionary alive but borrows nothing
# exclusively: an insertion while the view is live runs on both, and the
# view observes it, printing 1, 2, 9.
def main() raises:
    var d: Dict[Int, Int] = {1: 10, 2: 20}
    var kv = d.keys()
    d[9] = 90
    for k in kv:
        print(k)
