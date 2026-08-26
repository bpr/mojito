# Benchmark: String building, concatenation, find, split, and number
# formatting through String conversion; accumulates length checksums.
def main() raises:
    var total_len: Int = 0
    var finds: Int = 0
    var i: Int = 0
    while i < 9000:
        var s: String = String("row-") + String(i) + ":" + String(i * i % 97)
        total_len += len(s)
        if s.find("7") >= 0:
            finds += 1
        i += 1

    var pieces_total: Int = 0
    var r: Int = 0
    while r < 300:
        var csv: String = String("alpha,beta,gamma,delta,") + String(r)
        var parts = csv.split(",")
        pieces_total += len(parts)
        for p in parts:
            total_len += len(p)
        r += 1

    var sample: String = String("sample:")
    var k: Int = 0
    while k < 8:
        sample += " " + String(k * k)
        k += 1

    print(sample)
    print("total_len:", total_len)
    print("finds:", finds)
    print("pieces:", pieces_total)
