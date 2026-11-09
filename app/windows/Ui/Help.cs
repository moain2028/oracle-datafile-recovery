// Help.cs - the ℹ️ texts: what each page is for, how to use it, and what the numbers mean. Arabic, plain words.
using System.Drawing;
using System.Windows.Forms;

namespace OdrApp.Ui
{
    public static class Help
    {
        public const string Home =
@"🏠 الرئيسية

هذه الصفحة تجمع كل ما يفعله البرنامج في مكان واحد وتوجّهك للخطوة الصحيحة:

• عندك ملفات .dbf وتريد الجداول؟ ← «الاسترجاع».
• عندك ملف مشكوك في سلامته؟ ← «التشخيص» يخبرك ماذا حدث له ولماذا وما الذي يمكن إنقاذه.
• عندك مجلد كامل فيه عشرات الملفات؟ ← «الفرز» يرتّبها من الأسوأ للأفضل خلال دقائق.
• ضاع ملف SYSTEM ولا تعرف أسماء الجداول؟ ← «البصمة» تقترح الأسماء من قاعدة سليمة مشابهة.

البرنامج للقراءة فقط: لا يكتب ولا يعدّل أي ملف من ملفاتك أبداً، ويحسب بصمة SHA-256 للملف قبل وبعد ليثبت ذلك.";

        public const string Recover =
@"📦 الاسترجاع — استخراج الجداول من ملفات .dbf

الخطوات:
١) أضف ملفات قاعدة البيانات. يجب أن يكون بينها ملف SYSTEM (مثل SYSTEM01.DBF) لأنه يحوي «القاموس»: أسماء الجداول والأعمدة وأنواعها.
٢) اضغط «قراءة الجداول». ستظهر كل الجداول؛ جداول النظام مخفية افتراضياً.
٣) حدّد الجداول ثم «استخراج». لكل جدول مجلد فيه ثلاثة ملفات:

   • verified.csv  — موثَّق ✔: صفوف اجتازت كل الفحوص (checksum، بنية الكتلة، فكّ كل خلية بدقة). يمكن الاعتماد عليها.
   • inferred.csv  — مستنتج: صفوف قُرئت لكن أحد الأدلة ناقص (كتلة بلا checksum، رأس ملف ضائع…). راجعها قبل الاعتماد.
   • rejected.csv  — مرفوض: صفوف لم يمكن إثباتها، مع سبب الرفض لكل صف. لا نخمّن أبداً.

«استخراج بدون ملف SYSTEM»: إن ضاع القاموس يستخرج البرنامج كل كائن برقمه (OBJ_12345) بأعمدة COL_1.. وأنواع مستنتجة. استخدم صفحة «البصمة» لاقتراح الأسماء.

انقر مرتين على أي نتيجة لعرض بياناتها، ومنها «فتح في Excel».";

        public const string Diagnose =
@"🩺 التشخيص — ماذا حدث لهذا الملف؟

اختر ملف .dbf واحداً واضغط «تشخيص». البرنامج يفحص كل كتلة (8 كيلوبايت عادةً) ويحكم عليها:
سليمة / فارغة / أصفار / مشفَّرة / مقطوعة (torn) / checksum خاطئ / في غير مكانها / تلف منطقي / حمولة مشفَّرة برأس سليم / قديمة (SCN) / مكرَّرة.

ثم يحوّل الأرقام إلى «نتائج» مفهومة، لكل نتيجة:
   • الدليل: الأرقام التي بُنيت عليها (لا رأي بلا رقم).
   • السبب المرجّح: فدية؟ انقطاع كهرباء؟ قرص تالف؟ استرجاع ناقص؟
   • ما الذي يمكن استرجاعه وكيف، وأخطاء أوراكل التي كانت ستظهر (ORA-01578…).

الدرجة 0–100: كم من الملف قابل للاسترجاع من هذا الملف وحده. وبجانبها «الخسارة المتوقعة ٪» بالبايت.
   90–100 ممتاز · 70–89 جيد · 40–69 جزئي · 10–39 ضعيف · 0–9 ضائع

«حفظ تقرير HTML»: تقرير عربي قابل للطباعة يُسلَّم للعميل أو للتأمين أو للجهة القانونية، فيه بصمة الملف قبل وبعد.
«DB3F»: صيغة جنائية قياسية (DFRWS 2019) لكل كتلة — للمحققين.

تنبيه: «checksum سليم» لا يعني «محتوى سليم». البرنامج يكشف التشفير الذي يحافظ على رأس الكتلة، وهو ما يمرّ على dbv كأنه سليم.";

        public const string Triage =
@"⚖️ الفرز — ترتيب ملفات كثيرة من الأسوأ إلى الأفضل

اختر مجلداً (أو عدة ملفات). البرنامج يشخّص كل ملف ويعرضها في جدول واحد مرتّباً بالدرجة من الأسوأ للأفضل، مع:
الحجم، حجم الكتلة، عدد الكتل التالفة، أبرز نتيجة، والخسارة المتوقعة.

الملفات التي ليست أوراكل تُدرج مع السبب ولا تُتجاهَل بصمت.

متى تستخدمه؟ بعد حادثة (فدية، عطل قرص، انقطاع كهرباء) حين تصلك عشرات الملفات ولا تعرف من أين تبدأ: الفرز يعطيك قائمة أولويات خلال دقائق.

انقر مرتين على أي صف لفتح تشخيصه الكامل.";

        public const string Fingerprint =
@"🔍 البصمة — تسمية الجداول حين يضيع القاموس

حين يضيع ملف SYSTEM تظهر الجداول كأرقام (OBJ_12345) وأعمدتها COL_1.. بلا أسماء.
البصمة تحلّ ذلك بخطوتين:

١) «تعلُّم» من قاعدة سليمة لنفس المنتج (مثلاً نسخة أخرى من نفس نظام YemenSoft): يقرأ البرنامج قاموسها ويحفظ «شكل» كل جدول (عدد الأعمدة وأنواعها بالترتيب) في «كتاب بصمات». لا يحفظ أي بيانات.

٢) «مطابقة» تقرير الاستخراج بدون SYSTEM مع كتاب البصمات: لكل كائن يقترح اسم الجدول الأقرب مع درجة ثقة ٪.

مهم: هذه اقتراحات لا حقائق. درجة 95٪ تعني «الشكل متطابق جداً»، لكن القرار النهائي لك بعد النظر في البيانات.";

        public const string Settings =
@"⚙️ الإعدادات وحول البرنامج

• مجلد النتائج الافتراضي، ولغة التقارير (عربي/إنجليزي).
• معرّف هذا الجهاز: رقم ثابت لهذا الجهاز (8 خانات) يُستخدم للترخيص فقط، ولا يُرسل لأي خادم. إن طُلب منك، انسخه وأرسله للمطوّر.
• حالة الترخيص: يتحقق البرنامج عند التشغيل ثم كل 10 دقائق. بلا إنترنت يبقى على آخر حالة معروفة.
• الإصدار ومحرك odr.exe وبصمة توقيعه.

البرنامج من تطوير المهندس معين العباسي — للدعم: +967770941666";

        public static void Show(IWin32Window owner, string title, string text)
        {
            var f = new Form
            {
                Text = title, Size = new Size(640, 560), StartPosition = FormStartPosition.CenterParent, RightToLeft = RightToLeft.Yes, RightToLeftLayout = true,
                Font = Theme.Base, BackColor = Theme.Bg, MinimizeBox = false, MaximizeBox = false, ShowInTaskbar = false, FormBorderStyle = FormBorderStyle.FixedDialog
            };
            var head = new Panel { Dock = DockStyle.Top, Height = 56, BackColor = Theme.Navy };
            head.Controls.Add(new Label { Text = title, ForeColor = Color.White, Font = Theme.H2, AutoSize = true, Location = new Point(16, 14) });
            var box = new TextBox
            {
                Multiline = true, ReadOnly = true, ScrollBars = ScrollBars.Vertical, Dock = DockStyle.Fill, BorderStyle = BorderStyle.None,
                BackColor = Color.White, Text = text.Replace("\n", "\r\n"), Font = new Font("Segoe UI", 11f), Margin = new Padding(0)
            };
            var wrap = new Panel { Dock = DockStyle.Fill, Padding = new Padding(16), BackColor = Color.White };
            wrap.Controls.Add(box);
            var foot = new FlowLayoutPanel { Dock = DockStyle.Bottom, Height = 52, FlowDirection = FlowDirection.LeftToRight, Padding = new Padding(10) , BackColor = Theme.Bg};
            var ok = Theme.Primary("فهمت", 120); ok.Click += (s, e) => f.Close();
            foot.Controls.Add(ok);
            f.Controls.Add(wrap); f.Controls.Add(foot); f.Controls.Add(head);
            f.AcceptButton = ok;
            box.GotFocus += (s, e) => { box.SelectionLength = 0; ok.Focus(); };
            f.ShowDialog(owner);
        }
    }
}
