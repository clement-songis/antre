// ── Channel delivery outcomes ────────────────────────────────
//
// The gateway reports every outbox outcome on the `delivery` topic. Only a
// reply that could not be delivered needs the user's attention.

import { t } from "./i18n";
import type { DeliveryPayload } from "./types/ws-events";
import { showToast } from "./ui";

function channelName(channelType: string): string {
	if (!channelType) return t("chat:delivery.unknownChannel");
	return channelType.charAt(0).toUpperCase() + channelType.slice(1);
}

/** Show a toast when a channel reply is lost for good. Returns whether one was shown. */
export function handleDeliveryEvent(payload: DeliveryPayload): boolean {
	if (payload.state !== "failed") return false;
	showToast(
		t("chat:delivery.failed", {
			channel: channelName(payload.channelType),
			error: payload.error || t("chat:delivery.unknownError"),
		}),
		"error",
	);
	return true;
}
