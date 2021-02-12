import React from "react";

import { ErrorCard } from "components/common/ErrorCard";
import { useCluster } from "providers/cluster";
import { LoadingCard } from "components/common/LoadingCard";
import { TableCardBody } from "components/common/TableCardBody";
import { Epoch } from "components/common/Epoch";
import { Slot } from "components/common/Slot";

type Props = { epoch: string };

export function EpochDetailsPage({ epoch }: Props) {
  let output = <ErrorCard text={`Epoch ${epoch} is not valid`} />;

  if (!isNaN(Number(epoch))) {
    output = <EpochOverviewCard epoch={Number(epoch)} />;
  }

  return (
    <div className="container mt-n3">
      <div className="header">
        <div className="header-body">
          <h6 className="header-pretitle">Details</h6>
          <h2 className="header-title">Epoch</h2>
        </div>
      </div>
      {output}
    </div>
  );
}

function EpochOverviewCard({ epoch }: { epoch: number }) {
  const { epochSchedule, epochInfo } = useCluster();

  if (!epochSchedule || !epochInfo) { 
    return <LoadingCard message="Loading epoch" />;
  }

  const firstBlock = epochSchedule.firstNormalSlot + (epoch - epochSchedule.firstNormalEpoch) * epochSchedule.slotsPerEpoch;
  const lastBlock = epochSchedule.firstNormalSlot + (epoch + 1 - epochSchedule.firstNormalEpoch) * epochSchedule.slotsPerEpoch - 1;
  const currentEpoch = epochInfo.epoch;

  return (
    <>
      <div className="card">
        <div className="card-header">
          <h3 className="card-header-title mb-0 d-flex align-items-center">
            Overview
          </h3>
        </div>
        <TableCardBody>
          <tr>
            <td className="w-100">Epoch</td>
            <td className="text-lg-right text-monospace">
              <Epoch epoch={epoch} />
            </td>
          </tr>
          {epoch > 0 && (
            <tr>
              <td className="w-100">Previous Epoch</td>
              <td className="text-lg-right text-monospace">
                <Epoch epoch={epoch - 1} link />
              </td>
            </tr>
          )}
          {currentEpoch > epoch && (
            <tr>
              <td className="w-100">Next Epoch</td>
              <td className="text-lg-right text-monospace">
                <Epoch epoch={epoch + 1} link />
              </td>
            </tr>
          )}
          <tr>
            <td className="w-100">First block</td>
            <td className="text-lg-right text-monospace">
              <Slot slot={firstBlock} link />
            </td>
          </tr>
          <tr>
            <td className="w-100">Last block</td>
            <td className="text-lg-right text-monospace">
              <Slot slot={lastBlock} link />
            </td>
          </tr>
        </TableCardBody>
      </div>
    </>
  );
}
