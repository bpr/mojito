# Benchmark: a larger mixed program — structs, lists, strings, calls, and
# loops combined into a small inventory simulation with a formatted report.
@fieldwise_init
struct Item(Copyable, Movable):
    var name: String
    var qty: Int
    var price: Int

    def total(self) -> Int:
        return self.qty * self.price

    def restock(mut self, n: Int):
        self.qty += n

    def sell(mut self, n: Int) -> Int:
        var sold: Int = n
        if sold > self.qty:
            sold = self.qty
        self.qty -= sold
        return sold * self.price

@fieldwise_init
struct Ledger(Copyable, Movable):
    var revenue: Int
    var restocked: Int
    var transactions: Int

    def record_sale(mut self, amount: Int):
        self.revenue += amount
        self.transactions += 1

    def record_restock(mut self, n: Int):
        self.restocked += n
        self.transactions += 1

def make_name(i: Int) -> String:
    return String("item-") + String(i % 50)

def price_for(i: Int) -> Int:
    return 3 + (i * 7) % 23

def scramble(seed: Int) -> Int:
    return (seed * 1103515245 + 12345) % 2147483647

def build_inventory(count: Int, salt: Int) -> List[Item]:
    var items: List[Item] = []
    var i: Int = 0
    while i < count:
        items.append(Item(make_name(i + salt), 5 + i % 9, price_for(i + salt)))
        i += 1
    return items

def main():
    var ledger: Ledger = Ledger(0, 0, 0)
    var grand_total: Int = 0
    var sevens: Int = 0
    var name_bytes: Int = 0
    var seed: Int = 42

    var round_no: Int = 0
    while round_no < 10:
        var items: List[Item] = build_inventory(30, round_no)

        var k: Int = 0
        while k < len(items):
            seed = scramble(seed)
            var want: Int = seed % 7
            if want % 2 == 0:
                var amount: Int = items[k].sell(want)
                ledger.record_sale(amount)
            else:
                items[k].restock(want)
                ledger.record_restock(want)
            k += 1

        for j in range(len(items)):
            grand_total += items[j].total()
            name_bytes += len(items[j].name)
            if items[j].name.find("7") >= 0:
                sevens += 1
        round_no += 1

    var report: String = String("report:")
    var lines: Int = 0
    var b: Int = 0
    while b < 6:
        report += " [" + String(b) + "=" + String((grand_total + b) % 997) + "]"
        lines += 1
        b += 1

    print(report)
    print("rounds:", round_no, "lines:", lines)
    print("grand_total:", grand_total)
    print("revenue:", ledger.revenue)
    print("restocked:", ledger.restocked)
    print("transactions:", ledger.transactions)
    print("sevens:", sevens)
    print("name_bytes:", name_bytes)
