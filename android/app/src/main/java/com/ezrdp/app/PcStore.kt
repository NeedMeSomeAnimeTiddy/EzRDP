package com.ezrdp.app

import android.content.Context
import android.util.Base64
import org.json.JSONArray
import org.json.JSONObject

/** A PC this phone has paired with. */
data class SavedPc(
    val id: String,
    var name: String,
    val deviceId: ByteArray,
    val key: ByteArray,
    var home: String = "",
    var internet: String = "",
    var port: Int = 7270,
    /** Optional address typed by the user (e.g. a DDNS name), tried after the others. */
    var manual: String = "",
) {
    /** Addresses to try, best first: found on this Wi-Fi now, last home address, then internet. */
    fun addresses(liveHost: String?): List<Pair<String, Int>> {
        val list = ArrayList<Pair<String, Int>>()
        liveHost?.let { list.add(it to 6000) }
        if (home.isNotEmpty()) list.add(home to 1200)
        if (manual.isNotEmpty()) list.add(manual to 6000)
        if (internet.isNotEmpty()) list.add(internet to 6000)
        return list.distinctBy { it.first }
    }
}

/** Saved PCs, kept in app storage. */
class PcStore(context: Context) {
    private val prefs = context.getSharedPreferences("ezrdp_pcs", Context.MODE_PRIVATE)

    fun all(): MutableList<SavedPc> {
        val arr = JSONArray(prefs.getString("pcs", "[]"))
        return (0 until arr.length()).map {
            val o = arr.getJSONObject(it)
            SavedPc(
                id = o.getString("id"),
                name = o.getString("name"),
                deviceId = Base64.decode(o.getString("deviceId"), Base64.NO_WRAP),
                key = Base64.decode(o.getString("key"), Base64.NO_WRAP),
                home = o.optString("home"),
                internet = o.optString("internet"),
                port = o.optInt("port", 7270),
                manual = o.optString("manual"),
            )
        }.toMutableList()
    }

    fun get(id: String) = all().firstOrNull { it.id == id }

    private fun write(list: List<SavedPc>) {
        val arr = JSONArray()
        for (pc in list) {
            arr.put(
                JSONObject()
                    .put("id", pc.id)
                    .put("name", pc.name)
                    .put("deviceId", Base64.encodeToString(pc.deviceId, Base64.NO_WRAP))
                    .put("key", Base64.encodeToString(pc.key, Base64.NO_WRAP))
                    .put("home", pc.home)
                    .put("internet", pc.internet)
                    .put("port", pc.port)
                    .put("manual", pc.manual),
            )
        }
        prefs.edit().putString("pcs", arr.toString()).apply()
    }

    @Synchronized
    fun save(pc: SavedPc) {
        val list = all()
        val i = list.indexOfFirst { it.id == pc.id }
        if (i >= 0) list[i] = pc else list.add(pc)
        write(list)
    }

    @Synchronized
    fun remove(id: String) = write(all().filter { it.id != id })
}
