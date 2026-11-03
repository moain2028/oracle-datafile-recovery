// Csv.cs - RFC 4180 reader for odr's verified/inferred/rejected.csv (UTF-8, quoted fields, embedded newlines).
using System.Collections.Generic;
using System.IO;
using System.Text;

namespace OdrApp
{
    public static class Csv
    {
        /// <summary>Reads the header and up to maxRows data rows; total = number of data rows in the file.</summary>
        public static List<string[]> Read(string path, int maxRows, out int total)
        {
            var rows = new List<string[]>();
            total = 0;
            using (var r = new StreamReader(path, new UTF8Encoding(false), true))
            {
                var row = new List<string>();
                var f = new StringBuilder();
                bool q = false, any = false;
                int c;
                while ((c = r.Read()) != -1)
                {
                    char ch = (char)c;
                    any = true;
                    if (q)
                    {
                        if (ch == '"')
                        {
                            if (r.Peek() == '"') { f.Append('"'); r.Read(); }
                            else q = false;
                        }
                        else f.Append(ch);
                        continue;
                    }
                    if (ch == '"') { q = true; continue; }
                    if (ch == ',') { row.Add(f.ToString()); f.Clear(); continue; }
                    if (ch == '\r') continue;
                    if (ch == '\n')
                    {
                        row.Add(f.ToString()); f.Clear();
                        Emit(rows, row, maxRows, ref total);
                        row = new List<string>(); any = false;
                        continue;
                    }
                    f.Append(ch);
                }
                if (any) { row.Add(f.ToString()); Emit(rows, row, maxRows, ref total); }
            }
            return rows;
        }

        static void Emit(List<string[]> rows, List<string> row, int maxRows, ref int total)
        {
            if (rows.Count == 0) { rows.Add(row.ToArray()); return; } // header
            total++;
            if (rows.Count <= maxRows) rows.Add(row.ToArray());
        }

        /// <summary>Copy with a UTF-8 BOM so Excel shows Arabic correctly. Returns the copy's path.</summary>
        public static string ExcelCopy(string path)
        {
            var dst = Path.Combine(Path.GetDirectoryName(path), Path.GetFileNameWithoutExtension(path) + "_excel.csv");
            using (var o = new FileStream(dst, FileMode.Create, FileAccess.Write))
            using (var i = File.OpenRead(path))
            {
                o.Write(new byte[] { 0xEF, 0xBB, 0xBF }, 0, 3);
                i.CopyTo(o);
            }
            return dst;
        }
    }
}
